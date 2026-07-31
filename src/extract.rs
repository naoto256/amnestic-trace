//! Runs the extraction agent over {prior handoff, journal window} and checks
//! the result is usable. Anything short of a clean answer is an error, and
//! every caller of this module treats an error as "write nothing".

use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::journal::Host;

/// Shipped default. Nothing writes it to disk: an install that never
/// customizes anything has no `prompt.md`, so it tracks the binary. See
/// `Store::extraction_prompt` for what overrides it.
pub const DEFAULT_PROMPT: &str = include_str!("default-prompt.md");

/// Why a synthesize produced no new snapshot.
///
/// Two variants, because two is how many the caller distinguishes: whether this
/// was a failure at all. Nothing downstream reasons about recoverability, so
/// naming kinds of failure would claim a retry policy that does not exist.
/// Everything else belongs in the message, which is what the log prints.
#[derive(Debug)]
pub enum Failed {
    /// Nothing new in the journal. Not a failure — there was no work.
    Vacuous,
    /// Something went wrong. The message says what; nothing branches on it.
    Failed(String),
}

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failed::Vacuous => write!(f, "nothing new since the previous compaction"),
            Failed::Failed(m) => write!(f, "no snapshot written: {m}"),
        }
    }
}

impl From<io::Error> for Failed {
    fn from(e: io::Error) -> Self {
        Failed::Failed(e.to_string())
    }
}

/// A handoff longer than this is a runaway, not a summary of working memory.
const MAX_HANDOFF_BYTES: usize = 64 * 1024;
const MIN_HANDOFF_CHARS: usize = 20;

pub fn compose(prompt: &str, prior: Option<&str>, window: &str) -> String {
    format!(
        "{prompt}\n\n\
         ## Prior handoff\n\n{}\n\n\
         ## Session journal since the previous compaction\n\n{}\n",
        prior
            .filter(|p| !p.trim().is_empty())
            .unwrap_or("(none - this is the first compaction of this session)"),
        if window.trim().is_empty() {
            "(empty)"
        } else {
            window
        },
    )
}

/// An extraction that has not finished by now is wedged, not slow. Bounded so
/// the marker cannot stay `ongoing` forever and block every later delivery.
const EXTRACTION_TIMEOUT: Duration = Duration::from_secs(600);

/// Launches the CLI that produced this journal, since that is the one known to
/// be installed and authenticated in this environment.
///
/// Summarizing a transcript needs no tools, and the journal being summarized is
/// full of text written by whatever the session was working on — so an agent
/// that can run commands turns that text into an execution path.
///
/// How completely that is achieved differs by host, and the difference is worth
/// stating plainly rather than papering over:
///
/// - **Claude Code**: `--tools ""` makes the built-in tools unavailable, and
///   `--strict-mcp-config` with no config supplied leaves no MCP servers.
///   Verified by running it: the model can describe a command it would like to
///   run, and cannot run one.
/// - **Codex**: the agent cannot be disarmed. What the flags do achieve, and
///   what they do not, was measured under the exact flag set below.
///
///   Achieved: `features.shell_tool=false` removes the shell, and
///   `--sandbox read-only` blocks local writes — an `apply_patch` comes back
///   "writing is blocked by read-only sandbox".
///
///   Not achieved: the sandbox governs the local process, not the tools Codex
///   hosts or proxies. `mcp_servers={}` is passed and does nothing — upstream
///   bug openai/codex#16045, still open: an empty inline TOML table merges
///   non-destructively with the existing configuration, so every configured
///   server survives and no error is reported. Measured with both arms in the
///   same environment, one with the override and one without: identical, and a
///   canary file outside the working directory comes back either way.
///
///   The per-server form the upstream issue suggests,
///   `mcp_servers.<name>.enabled=false`, is deliberately not used. It needs the
///   name of every server the user has configured, which this tool cannot know
///   — and a list that misses one closes nothing while looking like it did.
///
///   Outbound network is **not** closed. The hosted web-fetch tool retrieved a
///   public URL under this flag set, and the agent's tool inventory included
///   tools that write to a remote host over SSH, send mail, and publish a
///   website. Those run outside the sandbox, so `read-only` does not reach
///   them.
///
///   So on Codex, journal text that successfully steers the extraction agent
///   can have it read any file the user can read *and get the contents off the
///   machine*. This is an accepted risk, not a solved problem: extraction still
///   runs on Codex, and the flags stay because each one removes something real.
///
/// `workdir` is an empty scratch directory in both cases, so nothing of this
/// tool's own — other sessions' handoffs, their keys, the prompt — is sitting
/// in reach of whatever does run.
pub fn run(host: Host, input: &str, workdir: &Path) -> Result<String, Failed> {
    let mut cmd = match host {
        Host::Claude => {
            let mut c = Command::new("claude");
            c.args([
                "-p",
                "--output-format",
                "text",
                // Availability, not pre-approval: `--allowedTools ""` would
                // approve nothing in advance while leaving every tool present
                // and callable.
                "--tools",
                "",
                // Ignore every configured MCP server. None is passed, so this
                // leaves the agent with none.
                "--strict-mcp-config",
            ]);
            c
        }
        Host::Codex => {
            let mut c = Command::new("codex");
            c.args([
                "exec",
                "--skip-git-repo-check",
                "--sandbox",
                "read-only",
                // Removes the shell tool. Verified: the agent reports having no
                // way to run a command.
                "-c",
                "features.shell_tool=false",
                // A no-op today: openai/codex#16045 — an empty inline TOML
                // table merges with the existing config rather than replacing
                // it, so every configured server survives and nothing errors.
                // Kept as a statement of intent that starts working by itself
                // once that is fixed. It is NOT a defence — see this function's
                // doc for what the Codex path actually allows.
                "-c",
                "mcp_servers={}",
                "-C",
            ]);
            c.arg(workdir);
            c.arg("-");
            c
        }
    };

    let mut child = cmd
        .current_dir(workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Inherited so the agent's own diagnostics land in the worker's log
        // rather than vanishing.
        .stderr(Stdio::inherit())
        .spawn()
        // Not installed, not on PATH, not executable: nothing about this window
        // is wrong, so the marker should survive for a later attempt.
        .map_err(|e| Failed::Failed(format!("could not start the extraction agent: {e}")))?;

    // Both pipes are serviced off-thread. The input is larger than a pipe
    // buffer and the output can be too, so writing and reading inline would
    // deadlock against a child doing the opposite.
    let mut sink = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("no stdin"))?;
    let payload = input.to_string();
    let writer = std::thread::spawn(move || sink.write_all(payload.as_bytes()));

    let mut source = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("no stdout"))?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        source.read_to_end(&mut buf).map(|_| buf)
    });

    let deadline = Instant::now() + EXTRACTION_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Failed::Failed("extraction agent timed out".into()));
        }
        std::thread::sleep(Duration::from_millis(200));
    };

    // The writer's error is deliberately ignored: a child that exits early
    // leaves a broken pipe here, and its exit status is the better diagnostic.
    let _ = writer.join();
    let stdout = reader
        .join()
        .map_err(|_| Failed::Failed("output reader panicked".into()))?
        .map_err(|e| Failed::Failed(format!("could not read the agent's output: {e}")))?;

    if !status.success() {
        // The agent ran and decided it could not do this. Feeding it the same
        // window again will reach the same place.
        return Err(Failed::Failed(format!(
            "extraction agent exited with {status}"
        )));
    }
    validate(&strip_preamble(&String::from_utf8_lossy(&stdout))).map_err(Failed::Failed)
}

/// Drops anything before the first `##` heading.
///
/// Told to emit only the handoff, the extraction agent still narrates its way
/// into it ("Looking at the journal... let me write this honestly"). That text
/// then becomes the memory a session wakes up holding, where reasoning about a
/// past task is indistinguishable from the task. The prompt defines the handoff
/// as beginning at its first section, so anything earlier is not part of it.
///
/// A prompt edited to drop the headings has no first section, and then this
/// leaves the output alone rather than emptying it. The prompt is the user's to
/// rewrite, so that case is reachable.
fn strip_preamble(raw: &str) -> String {
    let text = raw.trim_start();
    // Checked before searching for a heading mid-text, or the search finds the
    // *second* section and cuts the first one away.
    if text.starts_with("## ") {
        return text.to_string();
    }
    match text.find("\n## ") {
        Some(i) => text[i + 1..].to_string(),
        None => raw.to_string(),
    }
}

/// The only gate between a flaky agent run and overwriting working memory.
pub fn validate(raw: &str) -> Result<String, String> {
    let text = raw.trim();
    if text.chars().count() < MIN_HANDOFF_CHARS {
        return Err("produced no usable handoff".into());
    }
    if text.len() > MAX_HANDOFF_BYTES {
        return Err("exceeds the handoff budget".into());
    }
    Ok(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_and_whitespace_output() {
        assert!(validate("").is_err());
        assert!(validate("   \n\t\n ").is_err());
    }

    #[test]
    fn rejects_a_stub_too_short_to_be_a_handoff() {
        assert!(validate("ok").is_err());
    }

    #[test]
    fn rejects_a_runaway_that_would_blow_the_budget() {
        assert!(validate(&"x".repeat(MAX_HANDOFF_BYTES + 1)).is_err());
    }

    #[test]
    fn accepts_and_trims_a_plausible_handoff() {
        let out = validate("  \nStill fixing the retry loop in fetch().\n  ").unwrap();
        assert_eq!(out, "Still fixing the retry loop in fetch().");
    }

    #[test]
    fn narration_before_the_first_section_is_dropped() {
        let raw = "This is a first-compaction request. Let me look at the journal.\n\n\
                   I must not fabricate work that did not happen.\n\n\
                   ## Rules and rulings\nnone\n";
        let out = strip_preamble(raw);
        assert!(out.starts_with("## Rules and rulings"), "got: {out}");
        assert!(!out.contains("Let me look"));
    }

    #[test]
    fn output_that_already_starts_at_a_section_is_untouched() {
        let raw = "## Rules and rulings\nnone\n";
        assert_eq!(strip_preamble(raw), raw);
    }

    #[test]
    fn a_well_formed_handoff_keeps_its_very_first_section() {
        // Standing rules are the single thing the handoff most needs to carry,
        // and they are in the first section.
        let raw = "## Rules and rulings\n- \"never use the Foo library\"\n\n\
                   ## Task map and position\nfixing the parser\n";
        let out = strip_preamble(raw);
        assert!(
            out.contains("never use the Foo library"),
            "lost the rules: {out}"
        );
        assert!(out.contains("## Task map and position"));
        assert_eq!(out, raw);
    }

    #[test]
    fn a_headingless_prompt_keeps_its_output_rather_than_losing_it() {
        let raw = "just a paragraph of handoff text with no headings at all";
        assert_eq!(strip_preamble(raw), raw);
    }

    #[test]
    fn only_the_leading_narration_goes_not_later_sections() {
        let raw = "preamble\n\n## Rules and rulings\nnone\n\n## Rejected\nnone\n";
        let out = strip_preamble(raw);
        assert!(out.contains("## Rejected"));
        assert!(out.starts_with("## Rules"));
    }

    #[test]
    fn compose_marks_first_compaction_explicitly() {
        let c = compose("PROMPT", None, "journal");
        assert!(c.contains("first compaction"));
        assert!(c.contains("journal"));
    }

    #[test]
    fn compose_carries_the_prior_handoff() {
        let c = compose("PROMPT", Some("carried over"), "journal");
        assert!(c.contains("carried over"));
        assert!(!c.contains("first compaction"));
    }
}
