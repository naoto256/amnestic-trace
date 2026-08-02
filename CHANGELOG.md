# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Pre-1.0 releases may introduce breaking changes freely as the storage layout and hook contract converge. After 1.0, changes will follow semver strictly.

## [Unreleased]

### Added — the memory is delivered at the first tool call, not the next prompt

A compaction fires mid-turn and no host can inject from a compaction hook, so
delivery waited for `UserPromptSubmit` — that is, for the user to speak again. A
session left working in the meantime finishes things the snapshot still calls
pending, and the memory lands describing the state before them. Thirty minutes
of work was observed in that gap, an implementation, its tests and its
deployment all completed after the snapshot was written and absent from it.

A `PreToolUse` hook now delivers during that stretch instead. The turn-start
hook stays as the backstop for turns that call no tools, and the marker decides
which of them gets there first, so neither injects the same memory twice.

The two behave differently when the snapshot is not ready. The turn-start hook
has someone waiting on it: it polls, then gives up for good. The tool-call hook
is only ever early — it leaves the debt standing and does not poll, because
stalling every tool call in a turn costs more than the memory is worth.

`amtr recall --hook-json <event>` shapes the output as `additionalContext` for
the named event. `PreToolUse` ignores plain stdout, so a hook that printed there
would discharge the marker and deliver nothing. The binary builds the JSON
because a handoff is machine-written prose full of quotes, backslashes and
newlines, and a shell assembling JSON around that is one unescaped byte from
delivering nothing at all.

### Changed — the handoff budget is measured in tokens against what a host will deliver

Hosts cap the model-visible part of a hook's output and spill the rest to a
file, giving the model a head-and-tail preview and a path. Oversized memory does
not arrive short: it arrives with its middle replaced, still shaped like a
handoff, and recovering it takes a tool call nothing obliges the model to make.
The spilled file is world-readable under the system temp directory.

Measured on Codex: 9,129 characters of ASCII arrived whole and 11,128 spilled,
matching the ~2,500 tokens per message that host documents. `validate` now
rejects a handoff estimated over 2,000 tokens instead of the old 64KB, which
bore no relation to anything. The estimate is deliberately crude — CJK at a
token per character, Latin at a quarter — because the two differ fourfold and
that is the difference that decides whether a handoff fits.

The extraction prompt asks for less than the ceiling, and the Codex manifest
raises `additionalContextLimit` as a second margin. Rejecting costs one
compaction its memory; spilling costs the middle of it without saying so.

### Changed — the AMTR key no longer travels with the memory

Restored memory used to arrive under `AMTR key: <key> — report this key to the
user.` Both halves of that line were mistakes.

A key is a capability rather than a name: adopting one MOVES the snapshot away
from the session that owns it, and moving is the default. Nothing about
continuing the work needs it, so a session that is never given one cannot pass
it on. The instruction was the more direct problem — it names no audience, and
a session whose correspondent is another agent will report to that agent. This
was observed: an agent on a message fabric relayed its key to its peers, doing
exactly what the line asked of it.

The header now names the tool and the snapshot's boundary instead, which is the
one fact a reader cannot recover from the handoff and the one that decides
whether to trust it. A snapshot is taken when compaction fires and delivered at
the next turn start; a session that keeps working in between — the normal case
on a host that compacts mid-turn — can complete everything the record calls
pending. The preamble now says so, and says that the visible conversation is
the newer of the two.

Removing the genuine key line also removes the exception from the reader's
rule: every key-shaped line it can see is now remembered text, with none to
tell it apart from.

### Added — `amtr key`, and a bare `/amtr` that names this session's snapshot

Reads back this session's own key, for a user who is handing the work to
another session. It reads the store rather than the model's context, because a
key names one snapshot and not a lineage: every compaction mints a new one, so
a remembered key can name a snapshot that no longer exists.

Also the answer to "did the memory actually come back?" — the restoring hooks
say nothing when they work, and the removed key line had been doing double duty
as the only visible sign that one had run.

The skill spells it `/amtr` with nothing after it, which is the same command the
receiving end runs with a key. Holding a key or not is the entire difference
between the two ends of a handoff, so it needs no word of its own — and a verb
like `transfer` would have overstated it, since this end cannot move anything.
Only the receiving session can write its own row.

## [0.1.0] - 2026-07-31

> working memory that survives compaction

### Added — replacement memory across a context boundary

`amtr synthesize` runs from a `PreCompact` hook: it records that a snapshot is
owed, detaches a worker by double fork, and returns, so extraction runs
alongside compaction instead of delaying it. The worker reads the journal since
the previous compaction, runs the extraction agent over that window plus the
prior handoff, and replaces the session's single stored row.

`amtr recall` reads that row back. Delivery happens at the next turn start,
because neither host can inject context from a compaction hook.

Cross-session handoff is `amtr recall --amtr-key <key>`, which moves the row to
the calling session so the giving session forgets, or copies it with `--clone`.
A clone carries no key, since a key names one snapshot rather than a lineage.

Both Claude Code transcripts and Codex rollouts are read, and the extraction
agent launched is whichever host produced the journal.

### Added — plugin for Claude Code and Codex

Installable on both hosts from one marketplace catalog at the repo root. Each
host has its own manifest and its own hook declaration, and both run the same
`tools/amtr-hook.sh`. The `/amtr` skill wraps the cross-session handoff.

The Claude Code declaration sets explicit hook timeouts; the Codex one does not,
because the unit of that field is unverified there and a wrong guess would kill
the hook outright.

### Added — extraction prompt as the customization surface

The default prompt is compiled into the binary. `~/.local/share/amtr/prompt.md`
overrides it if you create it, and nothing in this tool ever writes there — so
an upgrade cannot disturb a prompt you tuned, and an install that never
customizes one tracks the current default instead of being pinned to whichever
version it first ran. `amtr default-prompt` writes the current default to
stdout for anyone starting an edit. There is no config file, no environment
variable, and no flag: the caller is a hook, so that file is the whole
interface.

### Notes

The store is `~/.local/share/amtr` when `~/.local` exists and `~/.amtr`
otherwise, except that an existing store always wins — the rule is evaluated at
every process start, so a `~/.local` that appears later must not leave earlier
rows behind. A command that needs a session refuses an empty session id, since
an unset host variable would otherwise file a snapshot where nothing asks for
it. No configurable environment variable takes part in either.

What the extraction agent can reach differs by host, and on the Codex path it
is not closed: hosted tools and configured MCP servers run outside the sandbox.
`plugin/README.md` carries the measurements.

Stored memory is deliberately ephemeral. Every failure path writes nothing to
stdout, so the host injects nothing and the turn proceeds, on the principle that
the next compaction redoes the work.

The exit status distinguishes what happened, because the delivery hook depends
on it: `0` handed over a handoff, `1` had nothing to hand over or failed, `2`
was called wrong. A caller that discharges a snapshot should do so only on `0`.

[Unreleased]: https://github.com/naoto256/amnestic-trace/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/naoto256/amnestic-trace/releases/tag/v0.1.0
