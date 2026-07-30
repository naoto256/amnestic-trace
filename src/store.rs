//! One JSON row per session_id, plus markers. No database, no lock, no ledger.
//!
//! Base directory resolution is deliberately env-free: hooks are spawned by the
//! host and are not guaranteed to inherit a shell environment, so a variable
//! like XDG_DATA_HOME could resolve differently for the writer (a detached
//! worker) and the reader (a hook), which would look like memory loss. The rule
//! is a hardcoded two-way branch on the existence of `~/.local`.

use std::fs;
use std::io::{self, Write};
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
    pub fn at(base: PathBuf) -> io::Result<Store> {
        fs::create_dir_all(base.join(CORTEX))?;
        Ok(Store { base })
    }

    /// Editable in place; written once if absent and never overwritten.
    pub fn prompt_path(&self) -> PathBuf {
        self.base.join("prompt.md")
    }

    fn row_path(&self, session_id: &str) -> PathBuf {
        self.base.join(CORTEX).join(format!("{}.json", slug(session_id)))
    }

    /// Lives beside the row rather than in a directory of its own: the marker
    /// is a property of the session, not a separate subsystem.
    pub fn marker_path(&self, session_id: &str) -> PathBuf {
        self.base.join(CORTEX).join(format!("{}.marker", slug(session_id)))
    }

    pub fn load(&self, session_id: &str) -> Option<Row> {
        let raw = fs::read_to_string(self.row_path(session_id)).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// UPSERT. Temp file + atomic rename, so a reader never sees a torn row.
    pub fn save(&self, row: &Row) -> io::Result<()> {
        let path = self.row_path(&row.session_id);
        let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
        let body = serde_json::to_vec_pretty(row).map_err(io::Error::other)?;
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&body)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &path)
    }

    pub fn forget(&self, session_id: &str) -> io::Result<()> {
        match fs::remove_file(self.row_path(session_id)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    /// Directory scan: the row count is at most the session count.
    pub fn find_by_key(&self, amtr_key: &str) -> Option<Row> {
        for entry in fs::read_dir(self.base.join(CORTEX)).ok()?.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let row: Row = match fs::read_to_string(&path).ok().and_then(|r| serde_json::from_str(&r).ok()) {
                Some(r) => r,
                None => continue,
            };
            if row.amtr_key.as_deref() == Some(amtr_key) {
                return Some(row);
            }
        }
        None
    }

    /// MOVE: the row's session_id becomes the caller's and the giving session
    /// forgets, so its next synthesize is a first-compaction.
    pub fn take(&self, row: &Row, new_session_id: &str) -> io::Result<Row> {
        let moved = Row { session_id: new_session_id.to_string(), ..row.clone() };
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
    /// `ongoing` means extraction is in flight; `ready` means a row is waiting
    /// to be injected. The reader deletes it once delivery succeeds, so a
    /// snapshot that lands while nobody is looking is still delivered at the
    /// next turn rather than lost.
    pub fn mark_ongoing(&self, session_id: &str) -> io::Result<()> {
        fs::write(self.marker_path(session_id), b"ongoing")
    }

    /// Atomic rename: a reader polling this path never observes a half-written
    /// state word, so it either keeps waiting or delivers, never both.
    pub fn mark_ready(&self, session_id: &str) -> io::Result<()> {
        write_atomic(&self.marker_path(session_id), b"ready")
    }

    /// Test-only: at runtime the reader is the hook script, which is shell, so
    /// nothing in the binary ever reads a marker back.
    #[cfg(test)]
    pub fn marker_state(&self, session_id: &str) -> Option<String> {
        fs::read_to_string(self.marker_path(session_id)).ok().map(|s| s.trim().to_string())
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
    pub fn extraction_prompt(&self, default: &str) -> String {
        let path = self.prompt_path();
        if let Ok(text) = fs::read_to_string(&path) {
            return text;
        }
        let _ = write_atomic(&path, default.as_bytes());
        default.to_string()
    }
}

fn write_atomic(path: &Path, body: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    fs::write(&tmp, body)?;
    fs::rename(&tmp, path)
}

/// Session ids are host-minted UUIDs in practice; this only guards against a
/// hostile or exotic id escaping the sessions directory.
pub fn slug(session_id: &str) -> String {
    let cleaned: String = session_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .collect();
    let cleaned = cleaned.trim_matches('.').to_string();
    if cleaned.is_empty() {
        "_".to_string()
    } else {
        cleaned
    }
}

pub fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// Volatile name of one snapshot. Milliseconds in base36 keep it short enough
/// for a human to read back over voice and monotonic enough to eyeball order;
/// the pid disambiguates two sessions compacting in the same millisecond,
/// which would otherwise make `find_by_key` pick an arbitrary one.
pub fn mint_key() -> String {
    let ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
    format!("amtr-{}-{}", base36(ms), base36(std::process::id() as u64))
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
        let got = s.load("a").unwrap();
        assert_eq!(got.handoff, "second");
        assert_eq!(got.amtr_key.as_deref(), Some("amtr-2"));
        assert!(s.find_by_key("amtr-1").is_none(), "superseded key must not resolve");
    }

    #[test]
    fn missing_session_reads_as_none() {
        assert!(scratch().load("nobody").is_none());
    }

    #[test]
    fn move_transfers_the_row_and_the_giver_forgets() {
        let s = scratch();
        s.save(&row("giver", Some("amtr-k"), "state")).unwrap();
        let moved = s.take(&s.find_by_key("amtr-k").unwrap(), "taker").unwrap();

        assert_eq!(moved.session_id, "taker");
        assert_eq!(s.load("taker").unwrap().handoff, "state");
        assert!(s.load("giver").is_none(), "giver's next synthesize must be a first-compaction");
        assert_eq!(s.find_by_key("amtr-k").unwrap().session_id, "taker");
    }

    #[test]
    fn move_onto_the_same_session_is_a_no_op_not_a_deletion() {
        let s = scratch();
        s.save(&row("same", Some("amtr-k"), "state")).unwrap();
        s.take(&s.find_by_key("amtr-k").unwrap(), "same").unwrap();
        assert_eq!(s.load("same").unwrap().handoff, "state");
    }

    #[test]
    fn clone_copies_drops_the_key_and_leaves_the_source_intact() {
        let s = scratch();
        s.save(&row("giver", Some("amtr-k"), "state")).unwrap();
        let copy = s.clone_to(&s.find_by_key("amtr-k").unwrap(), "taker", "2026-08-01T12:00:00.000Z").unwrap();

        assert_eq!(copy.handoff, "state");
        assert_eq!(copy.amtr_key, None, "a clone must not seed further key-based chaining");
        assert_eq!(copy.compacted_at, "2026-08-01T12:00:00.000Z");
        assert_eq!(s.load("giver").unwrap().handoff, "state", "clone is not a move");
        assert_eq!(s.find_by_key("amtr-k").unwrap().session_id, "giver");
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

        // The worker finishes; the debt becomes deliverable but stays owed.
        s.mark_ready("a").unwrap();
        assert_eq!(s.marker_state("a").as_deref(), Some("ready"));

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
        s.mark_ready("a").unwrap();
        assert_eq!(s.marker_state("a").as_deref(), Some("ready"), "debt must outlive the worker");
    }

    #[test]
    fn a_failed_extraction_leaves_no_debt() {
        let s = scratch();
        s.mark_ongoing("a").unwrap();
        s.unmark("a").unwrap(); // what synthesize does when work() errors
        assert_eq!(s.marker_state("a"), None, "nothing to deliver, so nothing owed");
    }

    #[test]
    fn a_late_worker_re_owes_after_a_timed_out_reader_gave_up() {
        let s = scratch();
        s.mark_ongoing("a").unwrap();
        s.unmark("a").unwrap(); // reader timed out and failed open
        s.mark_ready("a").unwrap(); // worker lands afterwards
        assert_eq!(s.marker_state("a").as_deref(), Some("ready"), "next turn delivers it");
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

    #[test]
    fn minted_keys_are_prefixed_and_non_empty() {
        let k = mint_key();
        assert!(k.starts_with("amtr-"));
        assert!(k.len() > 6);
    }
}
