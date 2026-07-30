//! Runs the extraction agent over {prior handoff, journal window} and checks
//! the result is usable. Anything short of a clean answer is an error, and
//! every caller of this module treats an error as "write nothing".

use std::io::{self, Write};
use std::process::{Command, Stdio};

use crate::journal::Host;

/// Shipped default. Materialized at the well-known path on first run;
/// customization is editing that file in place.
pub const DEFAULT_PROMPT: &str = include_str!("default-prompt.md");

/// A handoff longer than this is a runaway, not a summary of working memory.
const MAX_HANDOFF_BYTES: usize = 64 * 1024;
const MIN_HANDOFF_CHARS: usize = 20;

pub fn compose(prompt: &str, prior: Option<&str>, window: &str) -> String {
    format!(
        "{prompt}\n\n\
         ## Prior handoff\n\n{}\n\n\
         ## Session journal since the previous compaction\n\n{}\n",
        prior.filter(|p| !p.trim().is_empty()).unwrap_or("(none - this is the first compaction of this session)"),
        if window.trim().is_empty() { "(empty)" } else { window },
    )
}

/// Launches the CLI that produced this journal, since that is the one known to
/// be installed and authenticated in this environment.
pub fn run(host: Host, input: &str) -> io::Result<String> {
    let mut cmd = match host {
        Host::Claude => {
            let mut c = Command::new("claude");
            c.args(["-p", "--output-format", "text"]);
            c
        }
        Host::Codex => {
            let mut c = Command::new("codex");
            c.args(["exec", "--skip-git-repo-check", "-"]);
            c
        }
    };
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    child.stdin.take().ok_or_else(|| io::Error::other("no stdin"))?.write_all(input.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!("extraction agent exited with {}", out.status)));
    }
    validate(&String::from_utf8_lossy(&out.stdout))
}

/// The only gate between a flaky agent run and overwriting working memory.
pub fn validate(raw: &str) -> io::Result<String> {
    let text = raw.trim();
    if text.chars().count() < MIN_HANDOFF_CHARS {
        return Err(io::Error::other("extraction produced no usable handoff"));
    }
    if text.len() > MAX_HANDOFF_BYTES {
        return Err(io::Error::other("extraction output exceeds the handoff budget"));
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
