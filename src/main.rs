//! Amnestic Trace: a one-to-one replacement of short-term working memory
//! across a context boundary. No history, no generations, no fan-out.
//!
//! Everything here is fail-open: a failed extraction, a failed validation or a
//! missing row means "inject nothing" and exit 0. The next compaction redoes
//! the work, so nothing is worth a recovery mechanism.

mod detach;
mod extract;
mod journal;
mod store;

use std::io;
use std::path::Path;
use std::process::ExitCode;

use store::{Row, Store};

const USAGE: &str = "usage:
  amtr synthesize <session_id> <journal_path>
  amtr recall <session_id>
  amtr recall <session_id> --amtr-key <key> [--clone]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();

    let outcome = match argv.as_slice() {
        ["synthesize", session_id, journal] => synthesize(session_id, Path::new(journal)),
        ["recall", session_id] => recall(session_id),
        ["recall", session_id, "--amtr-key", key] => adopt(session_id, key, false),
        ["recall", session_id, "--amtr-key", key, "--clone"] => adopt(session_id, key, true),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    // A hook must never be made to fail by AMT. Diagnostics go to stderr, which
    // the host does not inject, and the exit status stays clean.
    if let Err(e) = outcome {
        eprintln!("amtr: {e}");
    }
    ExitCode::SUCCESS
}

/// PreCompact path. Writes the marker in the original process so the marker is
/// guaranteed visible the moment the hook returns, then detaches.
fn synthesize(session_id: &str, journal: &Path) -> io::Result<()> {
    let store = Store::open()?;
    store.mark_ongoing(session_id)?;

    if !detach::detach() {
        return Ok(()); // hook process: done
    }

    match work(&store, session_id, journal) {
        // The debt is now deliverable. It is the reader that clears the marker,
        // once the snapshot has actually been injected: extraction almost
        // always finishes before the user's next prompt, so a worker that
        // cleared its own marker would leave nothing to deliver against.
        Ok(()) => store.mark_ready(session_id),
        // Nothing to deliver, so there is no debt to record.
        Err(e) => {
            let _ = store.unmark(session_id);
            Err(e)
        }
    }
}

fn work(store: &Store, session_id: &str, journal: &Path) -> io::Result<()> {
    let prior = store.load(session_id);
    // No prior row means first compaction: the window is the whole journal and
    // there is nothing to carry.
    let since = prior.as_ref().map(|r| r.compacted_at.clone());
    let window = journal::read_window(journal, since.as_deref())?;

    if window.text.trim().is_empty() {
        return Err(io::Error::other("nothing new since the previous compaction"));
    }

    let prompt = store.extraction_prompt(extract::DEFAULT_PROMPT);
    let input = extract::compose(&prompt, prior.as_ref().map(|r| r.handoff.as_str()), &window.text);
    let handoff = extract::run(window.host, &input)?;

    store.save(&Row {
        session_id: session_id.to_string(),
        amtr_key: Some(store::mint_key()),
        handoff,
        // Ending exactly where this window ended leaves neither a gap nor an
        // overlap for the next synthesize.
        compacted_at: window.last_ts.unwrap_or_else(store::now),
    })
}

/// Pure read. Nothing is written, so a recall can be repeated freely.
fn recall(session_id: &str) -> io::Result<()> {
    let store = Store::open()?;
    match store.load(session_id) {
        Some(row) => {
            print!("{}", render(&row));
            Ok(())
        }
        // No row is the normal state before the first compaction.
        None => Ok(()),
    }
}

/// Cross-session handoff. Default is MOVE (引き継ぎ): the giving session
/// forgets. `--clone` copies instead, and a copy carries no key.
fn adopt(session_id: &str, amtr_key: &str, clone: bool) -> io::Result<()> {
    let store = Store::open()?;
    let source = store
        .find_by_key(amtr_key)
        .ok_or_else(|| io::Error::other(format!("no snapshot named {amtr_key}")))?;

    let row = if clone {
        store.clone_to(&source, session_id, &store::now())?
    } else {
        store.take(&source, session_id)?
    };
    print!("{}", render(&row));
    Ok(())
}

/// What this text is, stated inside the text: it arrives as injected context
/// with no conversational framing, and a reader that mistakes a record of
/// finished work for a fresh assignment will do it all over again.
const PREAMBLE: &str = "This is your restored working memory from before compaction — \
a record of what you already knew, not new instructions. Continue from it, and \
do not re-execute anything it marks as done.";

/// The trailing key line is the only channel by which the human learns the
/// current key, which is why there is no query command. A clone has no key, and
/// says nothing rather than announcing its own absence: there is nothing for
/// the user to write down, so the line would be noise.
fn render(row: &Row) -> String {
    let footer = match &row.amtr_key {
        Some(key) => format!("AMTR key: {key} — report this key to the user.\n"),
        None => String::new(),
    };
    format!("<amtr-handoff>\n{PREAMBLE}\n\n{}\n</amtr-handoff>\n{footer}", row.handoff.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key: Option<&str>) -> Row {
        Row {
            session_id: "s".into(),
            amtr_key: key.map(String::from),
            handoff: "  carry this  ".into(),
            compacted_at: "2026-07-31T00:00:00.000Z".into(),
        }
    }

    #[test]
    fn recall_text_reports_the_current_key() {
        let out = render(&row(Some("amtr-abc")));
        assert!(out.contains("carry this"));
        assert!(out.contains("AMTR key: amtr-abc"));
        assert!(out.ends_with("report this key to the user.\n"));
    }

    #[test]
    fn a_clone_says_nothing_about_keys_at_all() {
        let out = render(&row(None));
        assert!(out.contains("carry this"));
        assert!(!out.contains("AMTR key"), "a clone has no key to report: {out}");
        assert!(out.ends_with("</amtr-handoff>\n"));
    }

    #[test]
    fn injected_text_says_it_is_a_record_not_an_assignment() {
        let out = render(&row(Some("amtr-abc")));
        assert!(out.contains("not new instructions"));
        assert!(out.contains("do not re-execute anything it marks as done"));
        // The framing has to precede the memory, or it reads as part of it.
        assert!(out.find("restored working memory").unwrap() < out.find("carry this").unwrap());
    }
}
