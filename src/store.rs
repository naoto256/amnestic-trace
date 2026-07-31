//! One JSON row per session_id, plus markers. No database, no lock, no ledger.
//!
//! Base directory resolution takes no *configuration* from the environment.
//! Hooks are spawned by the host with no guaranteed shell environment, so a
//! tunable like XDG_DATA_HOME could resolve differently for the writer (a
//! detached worker) and the reader (a hook), which would present as memory
//! loss. The rule is a hardcoded two-way branch on the existence of `~/.local`.
//!
//! The home directory itself is unavoidably environmental: `home_dir()` reads
//! `$HOME` and falls back to the passwd entry. Both halves resolve it the same
//! way — the shell hook reads `$HOME` too — so they agree, which is the
//! property that actually matters here.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A snapshot of replacement memory for one session. There is at most one row
/// per session_id and it is overwritten in place: no generations, no history.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Row {
    pub session_id: String,
    /// Name of *this* snapshot, minted at every synthesize. `None` for a clone,
    /// which is not a synthesize product and must not seed further chaining.
    pub amtr_key: Option<String>,
    pub handoff: String,
    /// Boundary of the last window that was folded into `handoff`. The next
    /// synthesize reads the journal strictly after this timestamp.
    pub compacted_at: String,
}

/// Machine-managed rows and markers; `prompt.md` is the human's half of the
/// home directory and sits outside it.
const CORTEX: &str = "prefrontal-cortex";

/// Resolved on-disk layout. Everything AMT owns lives under one base dir.
pub struct Store {
    base: PathBuf,
}

impl Store {
    /// `~/.local/share/amtr` when `~/.local` exists, else
    /// `~/.amtr`.
    pub fn base_dir() -> io::Result<PathBuf> {
        let home = std::env::home_dir()
            .ok_or_else(|| io::Error::other("cannot determine home directory"))?;
        Ok(if home.join(".local").is_dir() {
            home.join(".local/share/amtr")
        } else {
            home.join(".amtr")
        })
    }

    pub fn open() -> io::Result<Store> {
        Store::at(Store::base_dir()?)
    }

    /// Opens (and creates) a store rooted at an explicit base. Tests use this.
    ///
    /// The tree is owner-only. What it holds is a verbatim distillation of a
    /// working session — file paths, quoted decisions, sometimes the shape of
    /// unreleased work — so it deserves the same treatment as a private key
    /// rather than the default umask.
    pub fn at(base: PathBuf) -> io::Result<Store> {
        // Created 0700 in the first place where the platform allows it, rather
        // than created then tightened — the latter leaves a brief window at the
        // umask's permissions with the directory already in place.
        create_dir_private(&base)?;
        create_dir_private(&base.join(CORTEX))?;
        // Still applied afterwards, so a store from an older version (or one
        // whose parent already existed) is brought up to the same footing.
        restrict_dir(&base);
        restrict_dir(&base.join(CORTEX));
        Ok(Store { base })
    }

    /// The store's own directory. The worker moves here after detaching so it
    /// no longer stands in the project the session was working on.
    pub fn base(&self) -> &Path {
        &self.base
    }

    /// Editable in place; written once if absent and never overwritten.
    pub fn prompt_path(&self) -> PathBuf {
        self.base.join("prompt.md")
    }

    fn row_path(&self, session_id: &str) -> PathBuf {
        self.base
            .join(CORTEX)
            .join(format!("{}.json", slug(session_id)))
    }

    /// Lives beside the row rather than in a directory of its own: the marker
    /// is a property of the session, not a separate subsystem.
    pub fn marker_path(&self, session_id: &str) -> PathBuf {
        self.base
            .join(CORTEX)
            .join(format!("{}.marker", slug(session_id)))
    }

    /// `Ok(None)` means only "no row here yet", which is the ordinary state
    /// before a first compaction. A row that exists but cannot be read or
    /// parsed is an error: collapsing the two into `None` turned a corrupt
    /// snapshot into a silent first-compaction, discarding everything carried
    /// so far with nothing anywhere saying why.
    pub fn load(&self, session_id: &str) -> io::Result<Option<Row>> {
        let raw = match fs::read_to_string(self.row_path(session_id)) {
            Ok(raw) => raw,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        serde_json::from_str(&raw)
            .map(Some)
            .map_err(|e| io::Error::other(format!("stored row is not readable: {e}")))
    }

    /// UPSERT. Temp file + atomic rename, so a reader never sees a torn row.
    pub fn save(&self, row: &Row) -> io::Result<()> {
        let body = serde_json::to_vec_pretty(row).map_err(io::Error::other)?;
        write_atomic(&self.row_path(&row.session_id), &body)
    }

    pub fn forget(&self, session_id: &str) -> io::Result<()> {
        match fs::remove_file(self.row_path(session_id)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    /// Directory scan: the row count is at most the session count.
    ///
    /// A row that cannot be read is reported rather than skipped. This is the
    /// lookup behind a cross-session handoff, where the key was typed by a
    /// human off another session's output — so "no such key" and "the row
    /// holding that key is corrupt" lead to completely different next steps,
    /// and answering both with silence sends the user hunting for a typo that
    /// is not there.
    pub fn find_by_key(&self, amtr_key: &str) -> io::Result<Option<Row>> {
        for entry in fs::read_dir(self.base.join(CORTEX))?.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let raw = match fs::read_to_string(&path) {
                Ok(raw) => raw,
                Err(e) => {
                    eprintln!("{}: cannot read {}: {e}", now(), path.display());
                    continue;
                }
            };
            match serde_json::from_str::<Row>(&raw) {
                Ok(row) if row.amtr_key.as_deref() == Some(amtr_key) => return Ok(Some(row)),
                Ok(_) => {}
                Err(e) => eprintln!("{}: {} is not readable: {e}", now(), path.display()),
            }
        }
        Ok(None)
    }

    /// MOVE: the row's session_id becomes the caller's and the giving session
    /// forgets, so its next synthesize is a first-compaction.
    pub fn take(&self, row: &Row, new_session_id: &str) -> io::Result<Row> {
        let moved = Row {
            session_id: new_session_id.to_string(),
            ..row.clone()
        };
        self.save(&moved)?;
        if slug(&row.session_id) != slug(new_session_id) {
            self.forget(&row.session_id)?;
        }
        Ok(moved)
    }

    /// CLONE: copy instead of move. The source row is untouched; the copy has no
    /// amtr_key, and its window boundary is the clone time.
    pub fn clone_to(&self, row: &Row, new_session_id: &str, at: &str) -> io::Result<Row> {
        let copy = Row {
            session_id: new_session_id.to_string(),
            amtr_key: None,
            handoff: row.handoff.clone(),
            compacted_at: at.to_string(),
        };
        self.save(&copy)?;
        Ok(copy)
    }

    /// The marker is an undelivered debt, not a "compaction happened" flag.
    ///
    /// `ongoing` means extraction is in flight. `ready:<amtr_key>` means a
    /// specific snapshot is waiting to be injected — the key is part of the
    /// state, not decoration. Without it, two things that must be told apart
    /// look identical: the debt this run created, and a debt an earlier
    /// compaction left undelivered. Every write goes through the same atomic
    /// path, so a reader polling here sees one state word or another, never a
    /// half-written one.
    pub fn mark_ongoing(&self, session_id: &str) -> io::Result<Option<String>> {
        // The previous state is handed back so the caller can put it right if
        // this synthesize fails. Overwriting is correct when it succeeds — the
        // new snapshot supersedes the old — but on failure the old row is still
        // on disk, and destroying its marker orphans it forever.
        let prior = fs::read_to_string(self.marker_path(session_id)).ok();
        write_atomic(&self.marker_path(session_id), b"ongoing")?;
        Ok(prior)
    }

    pub fn mark_ready(&self, session_id: &str, amtr_key: &str) -> io::Result<()> {
        write_atomic(
            &self.marker_path(session_id),
            format!("ready:{amtr_key}").as_bytes(),
        )
    }

    /// A row written but never marked deliverable is unreachable: the reader
    /// only looks when a marker says to. Worth a few attempts before giving up,
    /// since the usual cause is transient.
    pub fn mark_ready_retrying(&self, session_id: &str, amtr_key: &str) -> io::Result<()> {
        let mut last = None;
        for attempt in 0..3 {
            match self.mark_ready(session_id, amtr_key) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    last = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(50 * (attempt + 1)));
                }
            }
        }
        Err(last.unwrap_or_else(|| io::Error::other("could not mark deliverable")))
    }

    /// Puts back whatever the marker said before this synthesize claimed it.
    ///
    /// The guard this replaces read the marker and removed it if it said
    /// `ongoing` — but `mark_ongoing` had already written exactly that, so the
    /// guard was reading its own handwriting and deleting an earlier
    /// compaction's undelivered `ready` regardless. The state to restore has to
    /// be captured before the overwrite; it cannot be inferred afterwards.
    pub fn restore_marker(&self, session_id: &str, prior: Option<&str>) -> io::Result<()> {
        match prior {
            Some(state) => write_atomic(&self.marker_path(session_id), state.as_bytes()),
            None => self.unmark(session_id),
        }
    }

    /// Test-only: at runtime the reader is the hook script, which is shell, so
    /// nothing in the binary ever reads a marker back.
    #[cfg(test)]
    pub fn marker_state(&self, session_id: &str) -> Option<String> {
        fs::read_to_string(self.marker_path(session_id))
            .ok()
            .map(|s| s.trim().to_string())
    }

    pub fn unmark(&self, session_id: &str) -> io::Result<()> {
        match fs::remove_file(self.marker_path(session_id)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    /// Materializes the shipped default prompt only when absent, and never
    /// overwrites an existing one: this file is the user's to edit, and it is
    /// the sole customization surface (no --prompt flag, no config).
    /// An empty file is not customization, it is a truncated write or a slip of
    /// the editor — and using it would launch the extraction agent over a whole
    /// transcript with no instructions at all, whose output then overwrites
    /// working memory. Falls back to the default and says so.
    pub fn extraction_prompt(&self, default: &str) -> String {
        let path = self.prompt_path();
        match fs::read_to_string(&path) {
            Ok(text) if !text.trim().is_empty() => return text,
            Ok(_) => {
                eprintln!(
                    "{}: {} is empty; using the built-in prompt",
                    now(),
                    path.display()
                );
                return default.to_string();
            }
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                eprintln!(
                    "{}: cannot read {}, using the built-in prompt: {e}",
                    now(),
                    path.display()
                );
                return default.to_string();
            }
            Err(_) => {}
        }
        if let Err(e) = write_atomic(&path, default.as_bytes()) {
            eprintln!("{}: could not write {}: {e}", now(), path.display());
        }
        default.to_string()
    }
}

/// Single writer for everything under the store. The temp file is created
/// owner-only *before* any bytes reach it, so the contents are never briefly
/// world-readable, and the rename carries those bits to the final name.
fn write_atomic(path: &Path, body: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(body)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

/// Creates a directory owner-only from the moment it exists.
///
/// `create_dir_all` honours the umask, so the tighten-afterwards approach has
/// the directory readable for as long as it takes to call `chmod`. Parents are
/// created first with the ordinary call — they are `~/.local/share` and the
/// like, which are not ours to restrict.
fn create_dir_private(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir(path)
    }
}

/// Best-effort: a store that exists but could not be tightened is still better
/// than no memory at all, and the caller cannot act on the failure anyway.
#[cfg(unix)]
fn restrict_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn restrict_dir(_path: &Path) {}

/// Session ids are host-minted UUIDs in practice; this only guards against a
/// hostile or exotic id escaping the sessions directory.
///
/// Substitution is per **byte**, not per character. The reader is `sed`, which
/// counts bytes, so counting characters here made the two disagree on anything
/// multibyte: `ünïcode` became `_n_code` for the writer and `__n__code` for the
/// reader, and the reader would look for a marker at a name the writer never
/// wrote. Bytes are the representation both sides can agree on without either
/// of them knowing about encodings.
pub fn slug(session_id: &str) -> String {
    let cleaned: String = session_id
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
                b as char
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('.').to_string();
    if cleaned.is_empty() {
        "_".to_string()
    } else {
        cleaned
    }
}

pub fn now() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// Volatile name of one snapshot. Milliseconds in base36 keep it short enough
/// for a human to read back over voice and monotonic enough to eyeball order.
///
/// The random tail is the part that matters: the key is the only thing standing
/// between another session and this session's memory, and a timestamp plus a
/// pid is guessable by anyone who knows roughly when a compaction happened.
pub fn mint_key() -> String {
    let ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
    format!("amtr-{}-{}", base36(ms), base36(random_u64()))
}

/// 64 bits from the OS. Falls back to the pid only if the kernel's generator is
/// somehow unreadable, which keeps a key minting rather than failing the whole
/// synthesize — a weak key still beats losing the snapshot.
fn random_u64() -> u64 {
    let mut bytes = [0u8; 8];
    match fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)) {
        Ok(()) => u64::from_le_bytes(bytes),
        Err(e) => {
            // Said out loud. Falling back to the pid makes the key guessable,
            // and a security property that degrades in silence is worse than
            // one that was never claimed.
            eprintln!(
                "{}: no CSPRNG available, key falls back to a guessable value: {e}",
                now()
            );
            std::process::id() as u64
        }
    }
}

fn base36(mut n: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_else(|_| "0".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    fn scratch() -> Store {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("amtr-test-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&dir);
        Store::at(dir).unwrap()
    }

    fn row(session: &str, key: Option<&str>, handoff: &str) -> Row {
        Row {
            session_id: session.into(),
            amtr_key: key.map(String::from),
            handoff: handoff.into(),
            compacted_at: "2026-07-31T00:00:00.000Z".into(),
        }
    }

    #[test]
    fn upsert_overwrites_rather_than_accumulating() {
        let s = scratch();
        s.save(&row("a", Some("amtr-1"), "first")).unwrap();
        s.save(&row("a", Some("amtr-2"), "second")).unwrap();
        let got = s.load("a").unwrap().unwrap();
        assert_eq!(got.handoff, "second");
        assert_eq!(got.amtr_key.as_deref(), Some("amtr-2"));
        assert!(
            s.find_by_key("amtr-1").unwrap().is_none(),
            "superseded key must not resolve"
        );
    }

    #[test]
    fn missing_session_reads_as_none() {
        assert!(scratch().load("nobody").unwrap().is_none());
    }

    #[test]
    fn move_transfers_the_row_and_the_giver_forgets() {
        let s = scratch();
        s.save(&row("giver", Some("amtr-k"), "state")).unwrap();
        let moved = s
            .take(&s.find_by_key("amtr-k").unwrap().unwrap(), "taker")
            .unwrap();

        assert_eq!(moved.session_id, "taker");
        assert_eq!(s.load("taker").unwrap().unwrap().handoff, "state");
        assert!(
            s.load("giver").unwrap().is_none(),
            "giver's next synthesize must be a first-compaction"
        );
        assert_eq!(
            s.find_by_key("amtr-k").unwrap().unwrap().session_id,
            "taker"
        );
    }

    #[test]
    fn move_onto_the_same_session_is_a_no_op_not_a_deletion() {
        let s = scratch();
        s.save(&row("same", Some("amtr-k"), "state")).unwrap();
        s.take(&s.find_by_key("amtr-k").unwrap().unwrap(), "same")
            .unwrap();
        assert_eq!(s.load("same").unwrap().unwrap().handoff, "state");
    }

    #[test]
    fn clone_copies_drops_the_key_and_leaves_the_source_intact() {
        let s = scratch();
        s.save(&row("giver", Some("amtr-k"), "state")).unwrap();
        let copy = s
            .clone_to(
                &s.find_by_key("amtr-k").unwrap().unwrap(),
                "taker",
                "2026-08-01T12:00:00.000Z",
            )
            .unwrap();

        assert_eq!(copy.handoff, "state");
        assert_eq!(
            copy.amtr_key, None,
            "a clone must not seed further key-based chaining"
        );
        assert_eq!(copy.compacted_at, "2026-08-01T12:00:00.000Z");
        assert_eq!(
            s.load("giver").unwrap().unwrap().handoff,
            "state",
            "clone is not a move"
        );
        assert_eq!(
            s.find_by_key("amtr-k").unwrap().unwrap().session_id,
            "giver"
        );
    }

    #[test]
    fn no_marker_means_nothing_is_owed() {
        let s = scratch();
        assert_eq!(s.marker_state("a"), None);
        s.unmark("a").unwrap();
        s.unmark("a").unwrap(); // clearing a debt that is not there is fine
        assert_eq!(s.marker_state("a"), None);
    }

    #[test]
    fn delivery_runs_ongoing_then_ready_then_gone() {
        let s = scratch();
        s.mark_ongoing("a").unwrap();
        assert_eq!(s.marker_state("a").as_deref(), Some("ongoing"));

        // The worker finishes; the debt becomes deliverable but stays owed, and
        // names which snapshot it owes.
        s.mark_ready("a", "amtr-k1").unwrap();
        assert_eq!(s.marker_state("a").as_deref(), Some("ready:amtr-k1"));

        // Only the reader, having injected, discharges it.
        s.unmark("a").unwrap();
        assert_eq!(s.marker_state("a"), None);
    }

    #[test]
    fn a_ready_snapshot_survives_until_someone_reads_it() {
        // The common case: extraction finishes long before the next prompt.
        // The marker must still be there, or the snapshot is never injected.
        let s = scratch();
        s.mark_ongoing("a").unwrap();
        s.mark_ready("a", "amtr-k1").unwrap();
        assert_eq!(
            s.marker_state("a").as_deref(),
            Some("ready:amtr-k1"),
            "debt must outlive the worker"
        );
    }

    #[test]
    fn a_failed_synthesize_does_not_discard_an_older_undelivered_snapshot() {
        // Goes through `mark_ongoing`, which is the whole point. The previous
        // version of this test called the withdraw path directly and passed
        // while the real sequence destroyed the older debt: `mark_ongoing` had
        // already overwritten `ready` with `ongoing`, so the guard that only
        // removed `ongoing` was reading its own handwriting.
        let s = scratch();
        s.save(&row("a", Some("amtr-old"), "earlier snapshot"))
            .unwrap();
        s.mark_ready("a", "amtr-old").unwrap();

        // A later compaction claims the marker, then fails.
        let prior = s.mark_ongoing("a").unwrap();
        assert_eq!(prior.as_deref(), Some("ready:amtr-old"));
        s.restore_marker("a", prior.as_deref()).unwrap();

        assert_eq!(
            s.marker_state("a").as_deref(),
            Some("ready:amtr-old"),
            "an earlier compaction's undelivered snapshot must survive"
        );
        assert_eq!(s.load("a").unwrap().unwrap().handoff, "earlier snapshot");
    }

    #[test]
    fn a_failed_synthesize_withdraws_its_own_claim_when_nothing_was_owed() {
        let s = scratch();
        let prior = s.mark_ongoing("a").unwrap();
        assert_eq!(prior, None, "nothing was owed before this run");
        s.restore_marker("a", prior.as_deref()).unwrap();
        assert_eq!(
            s.marker_state("a"),
            None,
            "leaving `ongoing` would make every later turn sit through the poll"
        );
    }

    #[test]
    fn an_empty_prompt_file_falls_back_rather_than_running_uninstructed() {
        let s = scratch();
        fs::write(s.prompt_path(), "   \n\n  ").unwrap();
        assert_eq!(s.extraction_prompt("BUILT-IN"), "BUILT-IN");
    }

    #[test]
    fn a_corrupt_row_is_an_error_not_a_silent_first_compaction() {
        let s = scratch();
        s.save(&row("a", Some("amtr-k"), "state")).unwrap();
        fs::write(s.base().join(CORTEX).join("a.json"), "{ truncated").unwrap();

        assert!(
            s.load("a").is_err(),
            "reporting this as absent would re-summarize the whole journal and \
             drop everything carried so far"
        );
        assert!(
            s.load("nobody").unwrap().is_none(),
            "genuinely absent is still Ok(None)"
        );
    }

    #[test]
    fn a_late_worker_re_owes_after_a_timed_out_reader_gave_up() {
        let s = scratch();
        s.mark_ongoing("a").unwrap();
        s.unmark("a").unwrap(); // reader timed out and failed open
        s.mark_ready("a", "amtr-k1").unwrap(); // worker lands afterwards
        assert_eq!(
            s.marker_state("a").as_deref(),
            Some("ready:amtr-k1"),
            "next turn delivers it"
        );
    }

    #[test]
    fn the_default_prompt_is_materialized_once_then_read_back() {
        let s = scratch();
        assert_eq!(s.extraction_prompt("DEFAULT"), "DEFAULT");
        assert!(s.prompt_path().exists());
        fs::write(s.prompt_path(), "EDITED IN PLACE").unwrap();
        assert_eq!(s.extraction_prompt("DEFAULT"), "EDITED IN PLACE");
    }

    #[test]
    fn slug_keeps_an_exotic_session_id_inside_the_cortex_directory() {
        assert_eq!(slug("019efc46-72c1-7aa2"), "019efc46-72c1-7aa2");
        let escaped = slug("../../etc/passwd");
        assert!(!escaped.contains('/'), "no separator survives: {escaped}");
        assert!(!escaped.starts_with('.'), "cannot climb out: {escaped}");
        assert_eq!(slug(".."), "_");
    }

    /// The reader is the hook script, in shell, so this rule exists twice. They
    /// are one contract: if they disagree, the reader looks for a marker at a
    /// path the writer never wrote, and the memory silently stops arriving with
    /// nothing anywhere reporting a failure.
    ///
    /// Executes the lines lifted out of the real hook script, so editing the
    /// script's rule without editing this one fails the build or the test.
    /// Restating the rule here would drift in exactly the way it is meant to
    /// catch.
    #[test]
    fn the_shell_reader_derives_the_same_filename_as_the_writer() {
        // Compile-time: moving or renaming the hook stops the build rather than
        // quietly leaving this test asserting against nothing.
        const HOOK: &str = include_str!("../plugin/tools/amtr-hook.sh");

        let derivation = extract_slug_derivation(HOOK);

        // Deliberately includes ids the hook's own character-class gate would
        // reject. Testing only ids that pass the gate would make this assert
        // something weaker than its name claims, and would leave the agreement
        // resting on that gate rather than on the two rules matching.
        let ids = [
            "019fb5b0-b8d6-7432-a1ae-d03d37b6b32a",
            "..",
            ".leading",
            "trailing.",
            "...",
            "a.b_c-d",
            ".",
            // Never reaches the derivation at runtime; included so the rules
            // agree on their own terms rather than by someone else's guard.
            "../../etc/passwd",
            "a/b",
            "a b",
            "a;rm -rf /",
            "//",
            "$(whoami)",
            // Multibyte. The list above was entirely ASCII, which meant this
            // test asserted agreement over exactly the inputs where the two
            // implementations could not disagree — while its own comment
            // claimed it established the rules match regardless of the gate.
            // Rust counted characters and sed counted bytes.
            "ünïcode",
            "セッション",
            "e\u{0301}combining",
            "emoji\u{1F600}id",
        ];

        for id in ids {
            let program = format!("session_id=\"$1\"\n{derivation}\nprintf '%s' \"$slug\"");
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(&program)
                .arg("sh")
                .arg(id)
                .output()
                .expect("sh is available");
            let from_shell = String::from_utf8_lossy(&out.stdout).to_string();
            assert_eq!(
                slug(id),
                from_shell,
                "writer and reader disagree on the filename for {id:?}"
            );
        }
    }

    /// Lifts the two lines that derive `slug` out of the hook script.
    ///
    /// Panics rather than returning nothing when they cannot be found: a test
    /// that quietly stopped exercising the real rule would restore exactly the
    /// blind spot it exists to remove.
    fn extract_slug_derivation(script: &str) -> String {
        let lines: Vec<&str> = script
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("slug=") || l.starts_with("[ -n \"$slug\" ]"))
            .collect();

        assert_eq!(
            lines.len(),
            2,
            "expected the assignment and its empty-string fallback in \
             plugin/tools/amtr-hook.sh, found {lines:?} — if the rule moved, \
             point this extractor at its new shape"
        );
        assert!(
            lines[0].contains("sed"),
            "the slug assignment no longer uses sed: {:?}",
            lines[0]
        );
        lines.join("\n")
    }

    #[test]
    fn minted_keys_are_prefixed_and_non_empty() {
        let k = mint_key();
        assert!(k.starts_with("amtr-"));
        assert!(k.len() > 6);
    }
}
