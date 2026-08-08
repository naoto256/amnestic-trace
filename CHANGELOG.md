# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Pre-1.0 releases may introduce breaking changes freely as the storage layout and hook contract converge. After 1.0, changes will follow semver strictly.

## [0.1.3] - 2026-08-08

> the tool calls right after a compaction are the ones made blind

Upgrading on Codex requires re-approving the plugin's hooks. The tool-call
hook's declared timeout changes, and Codex trusts hook definitions by hash, so
any change to them invalidates the old approval. An unapproved hook is skipped
silently — there is no error to notice, the memory just stops arriving.

### Changed — the tool-call deliverer waits once per compaction

The tool-call hook exists because a session that keeps working after a
compaction does so without its memory: every tool call in that stretch is made
blind. But it delivered only what was already finished and stepped aside
otherwise, so a hook added to shrink the blind stretch still let the calls at
its start run blind — and those are the dangerous ones. The first thing a
session that has just lost "confirm before pushing" does is push.

It now waits for an extraction in flight, on a budget of 25s per compaction
shared by every tool call that follows, rather than 25s per call. The first
call to find the extraction unfinished writes a deadline beside the marker;
calls arriving before it wait alongside, calls arriving after step aside. Per
debt rather than per call because polling on every call would stall the
session's real work for as long as extraction takes, and an extraction that had
died would stall it indefinitely — an unbounded cost against a bounded benefit.

This does not eliminate memory-less tool calls. It bounds them: the unbounded
"until the user next speaks" becomes "until the extraction finishes or 25s,
whichever is sooner". A deadline that reads back further ahead than one budget
is refused rather than waited out, since it cannot have been written by a clock
that agrees with this one, and ending that wait is not the host timeout's job.

### Fixed — a snapshot could be injected twice into the same turn

Delivery discharged the marker after injecting, which settled which hook owned
a snapshot but settled it too late to be exclusive: by then the handoff had
already reached the host. Two callers reading the same claim therefore both
injected it. Waiting on a shared deadline turns that from a coincidence into
the normal case, since every waiter wakes the instant the snapshot lands.

Ownership is now taken before injecting, by renaming the marker — an operation
exactly one caller can win. Whoever loses finds nothing to move and steps
aside. Holding the marker also makes the identity check answerable, so it moved
to immediately after the rename, where nothing else can change the answer.

A claim held for delivery is put back if nothing was injected, and the restore
is a hard link rather than a copy. A copy has a gap between testing the claim
and reading it, and the redirect creates the marker before the copy discovers
its input is gone — leaving a marker that names no snapshot, which every later
hook sees and none can discharge. A link either publishes the claim whole or
fails, and failing is right both ways it can fail: the claim was swept, or a
newer marker already holds the name and must not be displaced by an older claim
returning.

A hook killed between claiming and settling leaves its claim behind, and
nothing removed it. `PreCompact` now sweeps them, since a new compaction
supersedes whatever they were holding.

## [0.1.2] - 2026-08-08

> streaming the journal so the memory keeps arriving

### Fixed — the 128 MiB ceiling silently stopped snapshots on long sessions

`read_window` refused any journal larger than 128 MiB and slurped the rest
into a `String`. The refusal was silent from the host's point of view — the
hook exits successfully with no output either way — so a session that grew
past the ceiling kept compacting, and each compaction landed with no memory
update, on every subsequent compaction, until the session ended or the
journal shrank.

The ceiling was the wrong shape of defence. The window that comes out is
bounded by `MAX_WINDOW_CHARS` regardless of source size, so all the
file-size limit was really guarding against was `read_to_string` allocating
past what fits. The reader now streams the file line by line, and each
line is byte-capped by a `MAX_LINE_BYTES` of 32 MiB — enough for real
tool-output records, short enough that a runaway line cannot exhaust the
worker. Accumulated entries sit in a `VecDeque` capped by cumulative
character count at `MAX_WINDOW_CHARS * 2`; over-cap pushes pop from the
front, so memory is a small constant multiple of the tail regardless of how
big the journal has grown.

Fault handling is by kind, not uniform: invalid UTF-8 or an over-length
record is dropped locally and the reader continues, `ErrorKind::Interrupted`
retries in place, and any other I/O error is propagated all the way out.
The reader never returns an `Ok(Window)` for a run it did not finish — the
first shape of the fix did, and hid the missing tail behind a successful
return.

## [0.1.1] - 2026-08-05

> what cannot cross whole crosses as a key

Upgrading on Codex requires re-approving the plugin's hooks: this release adds
a third hook, and Codex trusts hook definitions by hash, so every change to
them invalidates the old approval. An unapproved hook is skipped silently —
there is no error to notice, the memory just stops arriving.

### Changed — the extraction prompt knows what a ruling is, and what it may never drop

The shipped prompt asked for the right sections but left "obsolete" undefined,
and the extractor decided it per-turn: rulings set during one phase of work
vanished once the work moved on. Twenty-four measured compactions of one long
session showed the pattern — a project-scoped prohibition and a working-cadence
agreement survived 23 and 20 generations, while every phase-scoped ruling was
gone within six.

The pruning test is now explicit, and explicitly does not reach Rules and
rulings: a ruling leaves only by being superseded or shrunk. The budget rule
says the same from the other end — shrink before deleting, drop rulings last,
and a ruling too long to quote becomes a one-line key naming its topic, because
a key lets the waking session recover the words where an absence leaves nothing
to even miss. Task map now opens with what is being worked on and must name
what was NOT checked, since compaction lands mid-investigation and prose makes
a hypothesis read like a finding. And "the user" became "a principal" —
sessions run to another agent's brief as often as to a person's.

The injected preamble carries the reader's half of the same contract: the
memory is a compression, not a copy, and a line shrunk to a bare key is a place
to recover context from, not a gap to fill from plausibility.

### Fixed — the shared log no longer accumulates every project's memory

The extraction agent's stderr was inherited into the worker's log for
diagnostics, but the agent CLIs echo their final message there — the entire
handoff. Every successful extraction for every project on the machine was
appending its working memory to one plain-text file.

Stderr is now captured and discarded. Not on success only: failure is exactly
when there is a handoff on that stream to lose, because a run that timed out
mid-answer or produced a handoff too long to validate has already had it
echoed. The stream carries the agent's final message and its diagnostics
mixed together and nothing in the code can tell them apart, so the log gets
the byte count and a note that the content was withheld. The failure itself is
already named — could not start, timed out, exited with a status, failed
validation — and that is what the log is for.

The `/amtr` skill got the matching discipline: one command, its printed
result, and no rummaging through the store or the log when the answer is "no
snapshot".

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

[0.1.3]: https://github.com/naoto256/amnestic-trace/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/naoto256/amnestic-trace/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/naoto256/amnestic-trace/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/naoto256/amnestic-trace/releases/tag/v0.1.0
