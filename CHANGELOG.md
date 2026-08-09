# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Pre-1.0 releases may introduce breaking changes freely as the storage layout and hook contract converge. After 1.0, changes will follow semver strictly.

## [0.1.0] - 2026-08-09

> hooks name the event; Rust owns the memory protocol

### Added — replacement working memory across compaction

`amtr synthesize` accepts a host's PreCompact payload, detaches an extraction
worker, and reduces the journal since the previous boundary into one bounded
handoff. Each session owns one row that is overwritten rather than accumulated,
so the store remains ephemeral and the journal remains the source of truth.

`amtr recall SessionStart`, `PreToolUse`, and `UserPromptSubmit` are equal
injection opportunities over the same debt. Any event may open or join the one
25-second patience window, deliver the ready snapshot through an exclusive
atomic claim, or fold an unfinished debt after the deadline. A publication
racing with expiry is preserved for the next event.

The shell adapter is limited to canonical argument forwarding, minimal PATH
repair, stdin/stdout transport, and fail-open behavior. JSON parsing, store-path
resolution, deadline arithmetic, polling, claims, and cleanup live in Rust so
the protocol does not vary across sh, dash, bash, and ksh.

### Added — explicit handoff and read-only inspection

`amtr recall Handoff --amtr-key <key> [--clone]` moves or copies a named
snapshot into the current host session. `amtr key <session_id>` reveals the
current snapshot's capability only when a handoff is requested; injected memory
never carries the key.

`amtr peek` displays every matching snapshot together with its marker, deadline,
remaining wait, and orphan atomic-claim files. `--session-id` and `--amtr-key`
narrow the projection, while `--json` selects compact machine output.
Inspection does not create, repair, claim, or discharge store state. The
command exposes matching handoffs and AMTR keys and is therefore a local
same-user diagnostic, not a redacted sharing format.

### Security — private state and defensive boundaries

Store directories are created owner-only and machine-managed files are written
with mode 0600 on Unix. Host session identifiers are validated at capture,
delivery, and explicit-handoff boundaries; stored handoff text is escaped before
it enters the host's context frame. Journal fallback traversal skips unreadable
subtrees and symlinked directories.

The plugin declares exactly one PreCompact capture hook and three recall hooks,
with the event name passed explicitly to the Rust runtime. Hook failures emit no
diagnostic context and cannot fail the surrounding host event.

[0.1.0]: https://github.com/naoto256/amnestic-trace/releases/tag/v0.1.0
