# Amnestic Trace (amt)

A one-to-one replacement of short-term working memory across a context
boundary. Two cases only: a session surviving its own compaction, and an
explicit handoff to another session. What remains relevant is kept, the rest is
dropped, and the result overwrites what came before — there is no history, no
generations, and no shared memory.

## Commands

```
amt synthesize <session_id> <journal_path>   # PreCompact: detach and extract
amt recall <session_id>                      # pure read
amt recall <session_id> --amt-key <key>      # cross-session handoff (MOVE)
amt recall <session_id> --amt-key <key> --clone
```

`synthesize` writes a marker, self-daemonizes by double fork, and returns, so
extraction runs in parallel with compaction itself.

The marker is an **undelivered snapshot**, not a "compaction happened" flag:

```
ongoing  synthesize started       -> reader polls
ready    row written              -> reader injects, then deletes the marker
gone     delivered, or extraction failed, or the reader timed out
```

The distinction matters because extraction usually finishes long before the
user's next prompt. A worker that deleted its own marker on success would leave
the next turn with nothing to deliver against, so the snapshot would never be
injected — the marker has to outlive the worker and be discharged by whoever
consumes it. A worker that lands after a timed-out reader gave up rewrites
`ready`, and the turn after that delivers it.

Every failure exits 0 and writes nothing. The next compaction redoes the work.

## Home directory

`~/.local/share/amt/` if `~/.local` exists, otherwise `~/.amt/`. No environment
variable takes part in this: hooks are spawned by the host with no guaranteed
shell environment, and a path that resolved differently for the detached writer
and the reading hook would present as memory loss.

```
<amt home>/
  prompt.md                        # extraction prompt; written once, then yours
  prefrontal-cortex/
    <session_id>.json              # amt_key, handoff, compaction time
    <session_id>.marker            # extraction in flight
```

`prompt.md` is the only customization surface. There is no config file and no
`--prompt` flag, because the caller is a hook and nobody types the command.

## Install

```sh
cargo build --release
install -m 755 target/release/amt ~/.local/bin/amt
```

Claude Code: install `plugin/` as a plugin (`hooks/hooks.json` is picked up
automatically). Codex: copy `plugin/codex/hooks.json` to `~/.codex/hooks.json`
(merging with any existing hooks) and enable `[features] codex_hooks = true` in
`~/.codex/config.toml`.

Both hosts run the same `plugin/hooks/amt-hook.sh`; only the plugin-root
variable differs.

## Manual verification

The pure logic — window slicing, UPSERT/move/clone, output validation — is
covered by `cargo test`. The process-level behavior is not, because forking and
hook wiring cannot be asserted meaningfully in a unit test. Check it by hand:

**1. Detach really detaches.** With a real transcript path:

```sh
time amt synthesize test-detach ~/.claude/projects/<dir>/<uuid>.jsonl
ls ~/.local/share/amt/prefrontal-cortex/test-detach.marker   # exists immediately
pgrep -fl 'amt synthesize'                                   # worker still alive
```

The command must return in well under a second, the marker must already be on
disk when it does, and the worker must appear as a child of `init` (PPID 1) in
`ps -o ppid= -p <pid>`.

**2. The snapshot waits to be collected.** Let the worker finish, then check
that the debt is still recorded — this is the case that a self-clearing worker
would silently drop:

```sh
cat ~/.local/share/amt/prefrontal-cortex/test-detach.marker   # -> ready
```

`test-detach.json` must exist alongside it, and both must still be there
minutes later.

**3. Hook injection.** In a real session, force a compaction (`/compact`), wait
until the marker reads `ready`, then send a prompt — deliberately after a pause,
since that is the ordinary case. The handoff should appear in context, the
assistant should report the AMT key, and the marker should be gone afterwards.
Run Claude Code with `--debug hooks` to see the hook fire.

**4. Fail-open on timeout.** Write `ongoing` to a marker by hand for a live
session and send a prompt. The turn must proceed normally after ~25s with
nothing injected, and the marker must be gone.

**5. Codex ids agree.** The `/amt` skill keys rows by `$CODEX_THREAD_ID`, while
`synthesize` keys them by the `session_id` the hook receives. These must be the
same value or a handoff silently finds nothing. `CODEX_THREAD_ID` is known to
match the UUID in the session's own rollout filename; confirm the hook agrees by
logging its stdin once:

```sh
# temporarily, in amt-hook.sh: printf '%s' "$input" >> /tmp/amt-hook-stdin.log
# then compare the session_id there against echo $CODEX_THREAD_ID
```

**6. Handoff.** In session B, run `/amt <key from session A>`. B should receive
A's memory; A's row must be gone (`ls` the cortex directory). With `clone`, A's
row must survive and B's must have `"amt_key": null`.
