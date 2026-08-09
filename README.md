# Amnestic Trace (amtr)

[![CI](https://github.com/naoto256/amnestic-trace/actions/workflows/ci.yml/badge.svg)](https://github.com/naoto256/amnestic-trace/actions/workflows/ci.yml)
[![Release](https://github.com/naoto256/amnestic-trace/actions/workflows/release.yml/badge.svg)](https://github.com/naoto256/amnestic-trace/actions/workflows/release.yml)
[![GitHub release](https://img.shields.io/github/v/release/naoto256/amnestic-trace?sort=semver&display_name=tag)](https://github.com/naoto256/amnestic-trace/releases/latest)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

A one-to-one replacement of short-term working memory across a context
boundary. Two cases only: a session surviving its own compaction, and an
explicit handoff to another session. What remains relevant is kept, the rest is
dropped, and the result overwrites what came before — there is no history, no
generations, and no shared memory.

## What must survive

Carrying everything that matters, in full, is the goal — and it is not
attainable: the delivered memory is budgeted at about 2,000 tokens, and a
working session does not reduce to that without loss. So the design commits to
the next-best thing it can actually keep: **what cannot cross whole crosses as
a key.** A topic named is enough for the waking session to know the thing
existed and to go recover the words — from the journal, the repository, or the
user.

Compression is survivable; absence is not. A memory that leaves no fragment
leaves nothing to even miss, so nothing ever triggers the recovery — which
makes silent, total loss of a needed memory the one failure with no path back.
Everything downstream is this ranking applied: the extraction prompt shrinks
before it deletes and drops rulings last, and the injected preamble tells the
reader its memory is lossy and where to go for the rest.

## Commands

```
amtr synthesize                                      # PreCompact payload on stdin
amtr recall SessionStart                             # hook payload on stdin
amtr recall PreToolUse                               # hook payload on stdin
amtr recall UserPromptSubmit                         # hook payload on stdin
amtr recall Handoff --amtr-key <key> [--clone]       # explicit handoff
amtr peek [--session-id <id>] [--amtr-key <key>] [--json]
amtr key <session_id>                                # this session's own key
```

The hook-facing commands accept the host's JSON object intact. The shell
adapter does not parse it or touch delivery state; it only repairs the minimal
hook `PATH`, forwards stdin/stdout, and makes failures non-fatal to the host.
There are no legacy positional forms.

A key is a capability, not a name: whoever holds it can move a snapshot away
from the session that owns it, and moving is the default. So none is placed in
the injected memory — nothing about continuing the work needs one, and a
session wired into a channel of other agents cannot pass on what it was never
given. `amtr key` reads it back when a handoff is actually wanted, from the
store rather than from whatever a model remembers.

`synthesize` writes a marker, detaches by double fork, and returns, so
extraction runs in parallel with compaction itself.

`peek` is the debugging surface. With no filters it prints every stored row and
all delivery metadata, including the marker, deadline, remaining budget and
orphan claim files. `--session-id` and `--amtr-key` combine with AND. The
default is a labeled view for a person at a terminal; `--json` emits the same
fixed projection as compact JSON. It is a pure observation and does not create
a store or repair state. Both forms intentionally expose every matching
handoff and its AMTR key; `peek` is a local same-user diagnostic surface, not a
redacted sharing format.

The marker is an **undelivered snapshot**, not a "compaction happened" flag. It
names the snapshot it owes, so one debt can be told from another:

```
ongoing            synthesize started -> reader polls
ready:<amtr_key>   row written        -> reader injects, then deletes the marker
gone               delivered, or the attempt failed
```

The distinction matters because extraction usually finishes long before the
user's next prompt. A worker that deleted its own marker on success would leave
the next turn with nothing to deliver against, so the snapshot would never be
injected — the marker has to outlive the worker and be discharged by whoever
consumes it. A worker that lands after a timed-out reader gave up rewrites
`ready`, and the turn after that delivers it.

A failing synthesize deletes the marker and says so in the log. That is the
whole of it: the memory is ephemeral, so a failed extraction means there is no
memory this time, not that an older one is kept alive. The transcript survives
and the next compaction rebuilds from it.

Because the key is part of the marker, a reader discharges only the exact debt
it delivered. A snapshot that lands mid-turn is a different claim and survives.

## Three deliverers

A compaction fires in the middle of a turn, and nothing can be injected from
the `PreCompact` hook — at that moment the extraction has only just been handed
its input. So delivery falls to the hooks that run afterwards, and which one
lands the memory decides how stale it is when it arrives.

`SessionStart`, matched to `compact`, fires on both hosts the moment a
compaction ends — the earliest injection point there is. When the extraction
beat the compaction, the memory lands here, before the session does anything
else. When it did not, this hook opens the shared waiting window described
below rather than abandoning the debt.

`UserPromptSubmit` waits for the user to speak again. A session that keeps
working in between — the ordinary case for an agent left to run — can finish
everything the snapshot still calls pending. Half an hour of work has been
observed in that gap, with the memory arriving afterwards describing the state
before it.

`PreToolUse` runs throughout that stretch, so it delivers at the first tool
call after the snapshot lands:

```text
SessionStart      ready:<key> -> inject, discharge.   Fires as compaction ends.
                  ongoing     -> open or join this debt's shared deadline.
PreToolUse        ready:<key> -> inject, discharge.
                  ongoing     -> open or join the same deadline.
UserPromptSubmit  ready:<key> -> inject, discharge.
                  ongoing     -> open or join the same deadline.
```

All three share one 25s wall-clock budget per compaction debt. The first to
find an unfinished extraction writes a deadline of now + 25s; every arrival
before that deadline waits only for its remainder, and every arrival after it
waits no further. `SessionStart` usually opens the window because it runs first,
but `PreToolUse` or `UserPromptSubmit` opens it when either arrives first. One
debt therefore charges at most 25s of patience in total, no matter which hooks
pay it or how many of them arrive.

The deadline lives beside the marker and is cleared whenever the debt is — by
a new compaction, by delivery, or by whichever hook atomically folds an
extraction still unfinished after the window. A deadline that reads back more
than one budget ahead is refused rather than waited out: it cannot have been
written by a clock that agrees with this one, and ending that wait is not the
host timeout's job.

The three deliverers are deliberately equal after the shared wait as well as
inside it. Hook order is not a lifecycle: after compaction, `PreToolUse` and
`UserPromptSubmit` may arrive in either order and either may be followed by a
long autonomous stretch. Whichever first sees the deadline spent atomically
folds `ongoing`; a ready marker published in that cleanup race survives for the
next hook.

The 25s bounds deliberate stall, not the blind stretch. When extraction lands
inside the window, the waiting hook delivers before its host event continues.
When extraction outlives it, the hook at the deadline folds that debt and the
event continues without the memory. A worker that finishes after the atomic
fold publishes `ready:<key>` again, so a later hook can still collect the
snapshot without reopening the spent wait. Sharing one window prevents repeated
hooks from turning a bounded wait into an unbounded one without pretending the
deadline makes the memory available.

Whichever arrives first takes the marker — by renaming it, which exactly one
caller can win — and only the winner injects. Discharging it afterwards would
be too late to stop a second injection, since by then the handoff has already
gone to the host. Waiting on a shared deadline makes that race the normal case
rather than a coincidence: every waiter wakes the moment the snapshot lands.

## Size

Hosts cap the model-visible part of a hook's output and spill the rest to a
file, handing the model a head-and-tail preview and a path. An oversized handoff
therefore does not arrive short — it arrives with its middle replaced, in a
shape that still reads like a handoff, and recovering the rest takes a tool call
nothing obliges the model to make.

Measured on Codex: 9,129 characters of ASCII arrived whole, 11,128 spilled,
which puts the threshold at the ~2,500 tokens per message that host documents.
`validate` rejects a handoff estimated over 2,000 tokens rather than let one
through to be gutted, the extraction prompt asks for less than that, and the
Codex manifest raises `additionalContextLimit` as a second margin.

Rejecting costs one compaction its memory. Spilling costs the middle of it
without saying so.

Every failure writes nothing to stdout, so the host injects nothing and the turn
proceeds. The next compaction redoes the work.

The exit status still describes the CLI result: `0` handed over a handoff (or
printed a requested inspection), `1` had nothing to hand over or failed, and
`2` was called wrong. The hook adapter converts all of them to host success
after preserving any stdout, so AMTR cannot fail the surrounding event.

## Home directory

An existing `~/.amtr/` wins. Failing that, `~/.local/share/amtr/` if `~/.local`
exists, otherwise `~/.amtr/`. No *configurable* environment variable takes part:
hooks are spawned by the host with no guaranteed environment, and a tunable
that resolved differently across binary invocations would present as memory
loss. (`$HOME` itself is unavoidable.)

The existing store is checked first because this is resolved at every process
start. A machine whose `~/.local` did not exist at the first run keeps its rows
in `~/.amtr/`, and some unrelated program creating `~/.local` later must not
move the store away from them.

```
<home>/
  prompt.md                        # optional: yours if you create it
  amtr.log                         # detached worker's stderr, truncated at 256K
  prefrontal-cortex/
    <session_id>.json              # amtr_key, handoff, compaction time
    <session_id>.marker            # ongoing | ready:<amtr_key>
    <session_id>.deliver-deadline  # shared patience window, while outstanding
    <session_id>.*.<pid>.<seq>      # short-lived atomic claim candidates
```

The tree is created `0700` and every file in it `0600` — not because a handoff
is a secret, but because the store is where every session's handoff ends up at
once, and that is not something to leave to the ambient umask.

Rows also carry each snapshot's key, which is the one thing here that is not in
the journal the handoff came from.

None of that is protection in any stronger sense. A handoff is derived from a
journal the host already wrote to disk, and on Codex that journal is
world-readable, so anything running as you can read the source of every row
without going near this directory. Injected memory is written back into the
journal too, and hook output over the host's size limit is spilled to a file
under the system temp directory. Nothing here reaches any of those.

When memory stops arriving, `amtr.log` is the place to look — everything the
worker does happens after it has detached from any terminal, so this is the only
evidence it leaves.

`prompt.md` is the only customization surface. There is no config file and no
`--prompt` flag, because the caller is a hook and nobody types the command.

The default prompt is built into the binary and nothing writes `prompt.md` — an
install that never customizes it has no such file, and each upgrade brings its
own default. Create the file to override, starting from the current default if
you want one:

```sh
if [ -d "$HOME/.amtr" ]; then
  AMTR_HOME="$HOME/.amtr"
elif [ -d "$HOME/.local" ]; then
  AMTR_HOME="$HOME/.local/share/amtr"
else
  AMTR_HOME="$HOME/.amtr"
fi
mkdir -p "$AMTR_HOME"
amtr default-prompt > "$AMTR_HOME/prompt.md"
```

## Install

```sh
brew install naoto256/amnestic-trace/amtr
```

The formula takes the same release binary described below, with the same
checksums; the tap is [naoto256/homebrew-amnestic-trace](https://github.com/naoto256/homebrew-amnestic-trace).

To place that binary yourself instead, verify it first, substituting the
release you downloaded for `X.Y.Z`:

```sh
tar -xzf amtr-vX.Y.Z-aarch64-apple-darwin.tar.gz
sha256sum -c SHA256SUMS        # shasum -a 256 -c on macOS
mkdir -p ~/.local/bin          # install does not create it
install -m 755 amtr ~/.local/bin/amtr
```

Or build from source, which is also how you run a modified copy:

```sh
cargo install --path .
```

Then install the plugin, which wires the hooks that call the binary:

```sh
# Claude Code — from the published repo
claude plugin marketplace add naoto256/amnestic-trace
claude plugin install amtr@naoto256-amtr

# Codex
codex plugin marketplace add naoto256/amnestic-trace
codex plugin add amtr@naoto256-amtr
```

Substitute an absolute path for `naoto256/amnestic-trace` to install from a
local checkout instead.

Both hosts install from the same `plugin/` directory via
`.claude-plugin/marketplace.json` at the repo root. Codex additionally needs
hooks enabled, and its first session will ask you to trust them. Those steps,
plus uninstall and prerequisites, are in
[`plugin/README.md`](plugin/README.md) — the authority for anything
host-specific.

### Upgrading on Codex

Codex trusts hook definitions by hash, so a release that changes any of them
invalidates the approval you already gave, and the session after an upgrade
will ask again. Until it is answered the hooks do not run — and they do not say
so, because a hook that is never invoked cannot report anything. A compaction in
that window falls back to the host's own summary and nothing marks the
difference.

So after upgrading, check that the first session prompts for trust and answer
it. If it did not prompt and memory has stopped arriving, the release notes for
the version you moved to say whether its hooks changed; a release that changed
them and did not prompt has an approval left over from an install that is no
longer there.

Without the plugin the binary is still usable by hand, and the hooks are the
only thing that makes it automatic.

## Manual verification

The Rust tests cover window slicing, UPSERT/move/clone, validation and the
concurrent delivery protocol. `tests/hook-regressions.sh` replays the thin
adapter under sh, dash, bash and ksh, checking only its real responsibilities:
canonical argument forwarding, byte-preserving stdin/stdout, PATH repair and
fail-open behavior. What neither covers is host wiring. Check that by hand:

Set these first, since the store's location depends on the machine and the
angle brackets a placeholder would use are redirections to the shell:

```sh
if [ -d "$HOME/.amtr" ]; then
  AMTR_HOME="$HOME/.amtr"
elif [ -d "$HOME/.local" ]; then
  AMTR_HOME="$HOME/.local/share/amtr"
else
  AMTR_HOME="$HOME/.amtr"
fi
TRANSCRIPT=~/.claude/projects/PROJECT_DIR/SESSION_UUID.jsonl
```

**1. Detach really detaches.** With a real transcript path:

```sh
printf '{"session_id":"test-detach","transcript_path":"%s"}\n' "$TRANSCRIPT" |
  time amtr synthesize
ls "$AMTR_HOME/prefrontal-cortex/test-detach.marker"   # exists immediately
pgrep -fl 'amtr synthesize'                            # worker still alive
```

The command must return in well under a second, the marker must already be on
disk when it does, and the worker must appear as a child of `init` (PPID 1) in
`ps -o ppid= -p "$(pgrep -f 'amtr synthesize' | head -1)"`.

**2. The snapshot waits to be collected.** Let the worker finish, then check
that the debt is still recorded — this is the case that a self-clearing worker
would silently drop:

```sh
cat "$AMTR_HOME/prefrontal-cortex/test-detach.marker"   # -> ready:amtr-...
```

`test-detach.json` must exist alongside it, and both must still be there
minutes later.

**3. Hook injection.** In a real session, force a compaction (`/compact`), wait
until the marker reads `ready:<key>`, then send a prompt — deliberately after a
pause, since that is the ordinary case. The handoff should appear in context
under a line naming the snapshot's boundary, no key should appear outside the
`<amtr-handoff>` span, and the marker should be gone afterwards. A key-shaped
line *inside* the span is not a failure: the memory is written from a journal
that contains earlier injected ones, and the preamble tells the reader that
every such line is remembered text. Run Claude Code with
`--debug hooks` to see the hook fire.

Ask the assistant whether its memory was restored, and it should be able to
answer from that line — the tool is otherwise silent, so this is what makes a
working injection distinguishable from a hook that never ran.

**3b. The compaction-end and tool-call deliverers beat the turn-start one.** The
case they exist for: after a compaction, have the session keep working without
you saying anything — any task that runs a few tools. If the extraction finished
first, the memory should already be in context when the compaction ends;
otherwise it should arrive at the first tool call after the snapshot is ready.
Either way, not held until your next prompt. Check the marker is gone before you
speak again.

A hook that never delivers here is silent by design, so the marker is the
evidence: `ready:<key>` still sitting there while the session runs tools means
the tool-call path is not firing. On Codex that is usually trust — changing
anything in a hook definition, including a status message, invalidates the
approval, and an unapproved hook does not run and does not say so.

**4. Fail-open on timeout.** Write `ongoing` to a marker by hand for a live
session, then trigger any one of the three recall events. The event must proceed
normally after ~25s with nothing injected, and both marker and deadline must be
gone. Triggering either of the other events afterwards must not impose a second
wait for that debt.

**5. Codex ids agree.** The `/amtr` skill keys rows by `$CODEX_THREAD_ID`, while
`synthesize` keys them by the `session_id` the hook receives. These must be the
same value or a handoff silently finds nothing.

Confirm it without logging anything: in a live Codex session, write a row keyed
by `$CODEX_THREAD_ID` with a distinctive word in its handoff, mark it `ready`,
and ask the next turn to repeat that word. If it comes back, the hook resolved
the same id — the hook found the row *by* the id it was given.

Do not dump the hook's stdin to a file to check this. That payload carries the
user's prompt text, and a predictable path under `/tmp` is a poor place to put
it. If you must capture it, use `umask 077` and `mktemp`, and delete it after.

**6. Handoff.** In session A, run `/amtr` with no key to obtain its own — it is
not in A's context, so this is the only way to get it. Then in session B, run
`/amtr <that key>`. B should receive
A's memory; A's row must be gone (`ls` the cortex directory). With `clone`, A's
row must survive and B's must have `"amtr_key": null`.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
