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
    child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("no stdin"))?
        .write_all(input.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "extraction agent exited with {}",
            out.status
        )));
    }
    validate(&strip_preamble(&String::from_utf8_lossy(&out.stdout)))
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
/// leaves the output alone rather than emptying it.
fn strip_preamble(raw: &str) -> String {
    let text = raw.trim_start();
    // Already at the first section: there is nothing in front of it to drop.
    // This has to be checked before searching for a heading mid-text, or the
    // search finds the *second* section and the first one is cut away.
    if text.starts_with("## ") {
        return text.to_string();
    }
    match text.find("\n## ") {
        Some(i) => text[i + 1..].to_string(),
        None => raw.to_string(),
    }
}

/// The only gate between a flaky agent run and overwriting working memory.
pub fn validate(raw: &str) -> io::Result<String> {
    let text = raw.trim();
    if text.chars().count() < MIN_HANDOFF_CHARS {
        return Err(io::Error::other("extraction produced no usable handoff"));
    }
    if text.len() > MAX_HANDOFF_BYTES {
        return Err(io::Error::other(
            "extraction output exceeds the handoff budget",
        ));
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
        // Regression: searching for a heading mid-text finds the *second* one,
        // so obeying the prompt exactly cost the session its standing rules —
        // the single thing the handoff most needs to carry.
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
        // The prompt is the user's to rewrite; without sections there is no
        // boundary to cut at, and cutting everything would be worse.
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
