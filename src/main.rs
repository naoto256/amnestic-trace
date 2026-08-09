//! Amnestic Trace CLI routing.

mod detach;
mod extract;
mod journal;
mod peek;
mod recall;
mod store;
mod synthesize;

use std::io;
use std::process::ExitCode;

const USAGE: &str = "usage:
  amtr synthesize
  amtr recall <SessionStart|PreToolUse|UserPromptSubmit>
  amtr recall Handoff --amtr-key <key> [--clone]
  amtr peek [--session-id <id>] [--amtr-key <key>] [--json]
  amtr key <session_id>
  amtr default-prompt";

/// Whether this invocation produced output the host should apply.
///
/// The CLI uses `0` when it produced the requested output and `1` when it did
/// not (no debt, a marker that was folded, extraction still in flight past the
/// wait budget, or a genuine failure that was already logged). The hook adapter
/// preserves stdout but converts either status to host success, so the host
/// event itself never fails. `2` is reserved for an argv usage error.
/// `synthesize` always ends up at `Nothing`: its result is the detached worker,
/// not text for the host.
enum Status {
    Delivered,
    Nothing,
}

/// Keeps argv mistakes distinct from runtime `InvalidInput` without relying on
/// an error-message sentinel. The former is a usage banner and exit 2; the
/// latter is an ordinary failed operation and exit 1.
enum Error {
    Usage,
    Runtime(io::Error),
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Runtime(error)
    }
}

type Result<T> = std::result::Result<T, Error>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();

    let outcome = match argv.as_slice() {
        ["synthesize"] => synthesize::run()
            .map(|()| Status::Nothing)
            .map_err(Error::Runtime),
        ["recall", event] => match recall::Event::parse(event) {
            Some(event) => recall_hook(event),
            None => invalid_usage(),
        },
        ["recall", "Handoff", "--amtr-key", key] => handoff(key, false),
        ["recall", "Handoff", "--amtr-key", key, "--clone"] => handoff(key, true),
        ["peek", rest @ ..] => peek(rest),
        ["key", session_id] if !session_id.is_empty() => {
            print_optional(recall::report_key(session_id))
        }
        ["default-prompt"] => {
            print!("{}", extract::DEFAULT_PROMPT);
            Ok(Status::Delivered)
        }
        _ => invalid_usage(),
    };

    match outcome {
        Ok(Status::Delivered) => ExitCode::SUCCESS,
        Ok(Status::Nothing) => ExitCode::from(1),
        Err(Error::Usage) => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        Err(Error::Runtime(error)) => {
            eprintln!("amtr: {error}");
            ExitCode::from(1)
        }
    }
}

/// Hook-driven recall. stderr is redirected to the store's log first, because
/// the host discards a hook's stderr — printing errors would otherwise leave
/// no evidence anywhere. The interactive commands below intentionally do NOT
/// redirect: they are run at a terminal where the user is the audience.
fn recall_hook(event: recall::Event) -> Result<Status> {
    detach::log_stderr_to(&store::Store::base_dir()?);
    print_optional(recall::run(event))
}

/// Explicit cross-session handoff. Runs synchronously — the user typed this at
/// a shell and is waiting for the rendered snapshot — so no detach, no marker,
/// and errors surface to the invoking terminal rather than the log.
fn handoff(key: &str, clone: bool) -> Result<Status> {
    let output = recall::handoff(key, clone)?;
    print!("{output}");
    Ok(Status::Delivered)
}

fn print_optional(output: io::Result<Option<String>>) -> Result<Status> {
    match output? {
        Some(output) => {
            print!("{output}");
            Ok(Status::Delivered)
        }
        None => Ok(Status::Nothing),
    }
}

fn peek(args: &[&str]) -> Result<Status> {
    // Handwritten because the flag set is three and adding a parser crate for
    // this would be more code than the loop itself. Each flag can appear at
    // most once, and a repeated or empty-valued flag is a usage error rather
    // than a silent last-write-wins, which would make a mistyped diagnostic
    // command appear to have selected a well-defined session.
    let mut session = None;
    let mut key = None;
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        match args[index..] {
            ["--session-id", value, ..] if session.is_none() && !value.is_empty() => {
                session = Some(value);
                index += 2;
            }
            ["--amtr-key", value, ..] if key.is_none() && !value.is_empty() => {
                key = Some(value);
                index += 2;
            }
            ["--json", ..] if !json => {
                json = true;
                index += 1;
            }
            _ => return Err(Error::Usage),
        }
    }

    let records = peek::records(session, key)?;
    let output = peek::render(&records, json)?;
    println!("{output}");
    Ok(Status::Delivered)
}

fn invalid_usage() -> Result<Status> {
    Err(Error::Usage)
}
