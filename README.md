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

## Commands

```
amtr synthesize <session_id> <journal_path>   # PreCompact: detach and extract
amtr recall <session_id>                      # pure read
amtr recall <session_id> --amtr-key <key>      # cross-session handoff (MOVE)
amtr recall <session_id> --amtr-key <key> --clone
amtr key <session_id>                         # this session's own key
```

A key is a capability, not a name: whoever holds it can move a snapshot away
from the session that owns it, and moving is the default. So none is placed in
the injected memory — nothing about continuing the work needs one, and a
session wired into a channel of other agents cannot pass on what it was never
given. `amtr key` reads it back when a handoff is actually wanted, from the
store rather than from whatever a model remembers.

`synthesize` writes a marker, detaches by double fork, and returns, so
extraction runs in parallel with compaction itself.

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

Every failure writes nothing to stdout, so the host injects nothing and the turn
proceeds. The next compaction redoes the work.

The exit status says whether anything was delivered, because the caller has to
know: `0` handed over a handoff, `1` had nothing to hand over or failed, `2` was
called wrong. The reader discharges a snapshot only on `0`.

## Home directory

An existing `~/.amtr/` wins. Failing that, `~/.local/share/amtr/` if `~/.local`
exists, otherwise `~/.amtr/`. No *configurable* environment variable takes part:
hooks are spawned by the host with no guaranteed shell environment, and a
tunable that resolved differently for the detached writer and the reading hook
would present as memory loss. (`$HOME` itself is unavoidable, and both halves
read it the same way.)

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
```

The tree is created `0700` and every file in it `0600`. It holds a distillation
of a working session, which deserves the same handling as a private key.

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
amtr default-prompt > ~/.local/share/amtr/prompt.md
```

## Install

```sh
brew install naoto256/amnestic-trace/amtr
```

The formula takes the same release binary described below, with the same
checksums; the tap is [naoto256/homebrew-amnestic-trace](https://github.com/naoto256/homebrew-amnestic-trace).

To place that binary yourself instead, verify it first:

```sh
tar -xzf amtr-v0.1.0-aarch64-apple-darwin.tar.gz
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

Without the plugin the binary is still usable by hand, and the hooks are the
only thing that makes it automatic.

## Manual verification

The pure logic — window slicing, UPSERT/move/clone, output validation — is
covered by `cargo test`, and the hook script's own behavior by
`tests/hook-regressions.sh`, which replays its cases under every shell on the
machine that could be `/bin/sh`. What neither covers is the process level:
forking and hook wiring cannot be asserted meaningfully. Check that by hand:

Set these first, since the store's location depends on the machine and the
angle brackets a placeholder would use are redirections to the shell:

```sh
AMTR_HOME=~/.local/share/amtr      # or ~/.amtr; see "Home directory"
TRANSCRIPT=~/.claude/projects/PROJECT_DIR/SESSION_UUID.jsonl
```

**1. Detach really detaches.** With a real transcript path:

```sh
time amtr synthesize test-detach "$TRANSCRIPT"
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
under a line naming the snapshot's boundary, no key should appear anywhere in
it, and the marker should be gone afterwards. Run Claude Code with
`--debug hooks` to see the hook fire.

Ask the assistant whether its memory was restored, and it should be able to
answer from that line — the tool is otherwise silent, so this is what makes a
working injection distinguishable from a hook that never ran.

**4. Fail-open on timeout.** Write `ongoing` to a marker by hand for a live
session and send a prompt. The turn must proceed normally after ~25s with
nothing injected, and the marker must be gone.

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

**6. Handoff.** In session A, run `/amtr report` to obtain its key — it is not
in A's context, so this is the only way to get it. Then in session B, run
`/amtr <that key>`. B should receive
A's memory; A's row must be gone (`ls` the cortex directory). With `clone`, A's
row must survive and B's must have `"amtr_key": null`.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
