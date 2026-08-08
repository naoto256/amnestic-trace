# Amnestic Trace Plugin

Amnestic Trace replaces a session's short-term working memory across a context
boundary. This plugin wires the three hooks that make that automatic on both
Claude Code and Codex, and ships the `/amtr` skill for handing memory to another
session.

## What it does

- **`PreCompact` hook** (declared in both `hooks/claude.json` and
  `hooks/codex.json` → `tools/amtr-hook.sh precompact`).
  Hands the journal path to `amtr synthesize`, which records an undelivered
  snapshot and detaches a worker before returning. Extraction therefore runs in
  parallel with compaction itself rather than delaying it.
- **`PreToolUse` hook** (`tools/amtr-hook.sh deliver`). Delivers the snapshot at
  the first tool call after it is ready. Compaction happens mid-turn and no host
  can inject from a compaction hook, so something has to carry the memory
  forward; this is the earliest thing that runs. If extraction is still in
  flight it waits — but on a budget of 25s per compaction, shared by every tool
  call in the stretch, not 25s per call. The first call to find the extraction
  unfinished records a deadline next to the marker; calls arriving before it
  wait alongside, calls arriving after step aside. Either way the debt itself is
  left standing: abandoning it is the backstop's decision, not this hook's.
- **`UserPromptSubmit` hook** (`tools/amtr-hook.sh recall`). The backstop, for
  turns that call no tools at all. It waits while extraction is still running
  and gives up after 25s, injecting nothing and clearing the marker so later
  turns do not sit through the poll again. Its budget is per turn, because it
  only runs once a turn. On the delivering path the marker is cleared only once
  `amtr recall` reports that it actually printed a handoff — exit 0 means
  delivered, 1 means nothing was.
- **`/amtr` skill** (`skills/amtr/`). A thin wrapper over
  `amtr recall --amtr-key` for the cross-session case, plus a bare `/amtr` for
  reading back this session's own key. Compaction inside one session needs no
  key and no skill.

The two delivering hooks fire on the same turn once a snapshot is ready, and the
marker is what stops the memory being injected twice: whichever gets there first
discharges it, and it is discharged only against the exact claim that was
delivered, so a newer snapshot landing mid-turn survives.

Both emit `additionalContext` rather than printing to stdout. `PreToolUse`
ignores plain stdout on both hosts, so a hook that printed there would clear the
marker and deliver nothing — silently, on the path that was supposed to be the
fast one.

Both hosts run the same `tools/amtr-hook.sh`, but each declares it in its own
file: `hooks/claude.json` and `hooks/codex.json`, each named by the `hooks` key
in that host's manifest. Neither is found by convention, so the pairing is
stated rather than inferred. The script is shared because the work is
identical; the declarations are split because what can be asserted about each
host is not.

Concretely, the Claude Code file sets `timeout` explicitly — 10s for the capture
hook, which returns as soon as the worker has detached, and 35s for both
delivering hooks, each of which needs room for a 25s wait plus the read that
follows. The Codex file sets no
timeout, because the unit of that field is not documented for Codex and a wrong
guess would kill the hook instantly rather than fail visibly. Leaving it to the
host's default is the honest default.

The Codex file sets `additionalContextLimit` on both delivering hooks, which
Claude Code has no equivalent for. Codex caps model-visible hook output at
roughly 2,500 tokens and spills the rest to a file, handing the model a preview
and a path; memory delivered that way is a reference the model has to choose to
follow. The handoff is kept under that on its own — this is the margin, not the
mechanism.

## Prerequisites

- `amtr` on `PATH`:

  ```sh
  brew install naoto256/amnestic-trace/amtr
  ```

  `cargo install --path .` from the repo root works too, as does dropping a
  release binary in `~/.local/bin`. The hook script appends all three prefixes,
  plus `/usr/local`, because hook execution inherits a minimal `PATH` that omits
  them. Appended rather than prepended, so nothing here shadows the system's own
  tools.

- The host CLI that produced the journal (`claude` or `codex`) must be on
  `PATH` and authenticated — that is what performs the extraction. amtr reads
  the journal to decide which one to launch and never holds credentials itself.

## What the extraction agent can do

Summarizing needs no tools, so the agent is launched with as few as each host
allows. That is not the same amount on both, and the difference is worth
knowing before you install this.

The agent's input is a session journal, which contains text this tool did not
author — fetched pages, dependency output, error messages. Text like that can
try to steer whatever reads it.

- **Claude Code**: launched with `--tools ""`, so the built-in tools are
  unavailable rather than merely unapproved, and `--strict-mcp-config` with no
  config supplied leaves no MCP servers. Verified by running it: the model can
  describe a command it would like to run, and cannot run one.
- **Codex**: launched with `--sandbox read-only`, `-c features.shell_tool=false`
  and `-c mcp_servers={}`. Two of those three do something.

  **What they achieve.** The shell is genuinely gone, and local writes are
  genuinely blocked — an `apply_patch` comes back "writing is blocked by
  read-only sandbox".

  **What they do not.** `-c mcp_servers={}` is a no-op on current Codex — an
  upstream bug, [openai/codex#16045](https://github.com/openai/codex/issues/16045),
  still open. An empty inline TOML table merges with your existing
  configuration instead of replacing it, so every configured server survives
  and Codex reports no error. Measured with both arms in the same environment:
  identical, and a canary file outside the working directory came back either
  way.

  More importantly, the sandbox governs the *local process*. Codex's hosted
  tools and your configured MCP servers do not run inside it, so `read-only`
  says nothing about them. Measured under exactly this flag set: the hosted
  web-fetch tool retrieved a public URL successfully, and the agent's tool
  inventory included tools that write to a remote host over SSH, send mail, and
  publish a website.

  The per-server workaround in that issue,
  `-c mcp_servers.<name>.enabled=false`, is not used here: it needs the name of
  every server you have configured, which this tool cannot know, and a list
  that misses one would close nothing while appearing to close everything.

  When #16045 is fixed the override starts working on its own, and this section
  should be re-measured rather than assumed.

**What this means for the Codex path.** Journal text that successfully steers
the extraction agent can have it read any file you can read and **send the
contents off your machine**. It cannot write locally, and whatever it folds
into the handoff you would see in your next turn — but exfiltration does not
need the handoff, and the network path does not go through the sandbox.

This is stated so you can decide, not because it is fixed. Only one of the two
paths closes it: Claude Code, where the agent genuinely has no tools.

Pointing Codex at a `CODEX_HOME` that carries authentication and defines no MCP
servers removes one outbound route, not the class. Codex's own hosted tools are
not configured there and are not subject to the sandbox, so a second Codex home
narrows the exposure at the cost of maintaining it — it does not end it. Treat
any arrangement as partial until you have measured it the way described below.

To check your own setup, run the extraction command by hand against a canary
file outside the working directory and see whether it comes back. Phrase the
prompt as an instruction rather than a question: given an easy way to decline,
the agent may simply decline, and a file that was not read is not evidence that
it could not have been.

The agent runs in an empty temporary directory, deleted afterwards, so nothing
belonging to this tool — other sessions' handoffs, their keys, the prompt — sits
where it starts.

No daemon, no config file, and no environment variable. The extraction prompt is
built into the binary. Nothing is written to `~/.local/share/amtr/prompt.md`; if
you create that file, it is used instead — that is the whole customization
surface.

So an install that never customizes anything carries no prompt file, and each
upgrade brings its improved default along with it. To start from the current
default rather than a blank page:

```sh
amtr default-prompt > ~/.local/share/amtr/prompt.md
```

**Upgrades never touch that file.** Once it exists it is yours, and no version of
this tool writes there, so an upgrade cannot replace prompt text you tuned. The
cost is that later improvements to the default stop reaching you — re-run the
command above (or delete the file) to pick them up.

## Install

Both hosts discover plugins through marketplace catalogs, not by scanning
directories. The repo root carries a `.claude-plugin/marketplace.json` that
points at this `plugin/` subdirectory as the install source, and it serves both
hosts.

### Claude Code

```sh
claude plugin marketplace add naoto256/amnestic-trace
claude plugin install amtr@naoto256-amtr
```

From a local checkout during development:

```sh
claude plugin marketplace add /absolute/path/to/amnestictrace
claude plugin install amtr@naoto256-amtr
```

### Codex

```sh
codex plugin marketplace add naoto256/amnestic-trace
codex plugin add amtr@naoto256-amtr
```

From a local checkout:

```sh
codex plugin marketplace add /absolute/path/to/amnestictrace
codex plugin add amtr@naoto256-amtr
```

Codex reads `.codex-plugin/plugin.json` and, through it, `hooks/codex.json`
from this same directory. Hooks must also be enabled in `~/.codex/config.toml`:

```toml
[features]
hooks = true
```

(Older Codex builds called this `codex_hooks`; that spelling still loads but
warns that it is deprecated.)

Restart the session on either host so the hooks take effect. The first
interactive Codex session after installing will ask you to review and trust the
new hooks before it will run them — hooks run outside its sandbox, so Codex
requires a human to approve them and no amount of configuration skips that.

For a marketplace added from a local directory, Codex runs the plugin **from
that directory**, not from the copy under `~/.codex/plugins/cache/`. Editing the
source takes effect on the next session; editing the cache does nothing. A
Git-backed marketplace behaves the other way around and needs
`codex plugin marketplace upgrade naoto256-amtr` to pick up changes.

`codex exec` does fire the delivery hooks — it emits `UserPromptSubmit` and
`PreToolUse` like an interactive session. They simply have nothing to deliver: a
non-interactive run is its own session with its own id, so they find no marker
for it and exit without injecting. The same is true of the extraction subprocess
this tool launches, which is why that does not feed itself its own memory.

## Uninstall

```sh
claude plugin uninstall amtr@naoto256-amtr
codex plugin remove amtr@naoto256-amtr
```

Removing the plugin stops all capture and injection but leaves stored memory in
place. To discard that too:

```sh
rm -rf ~/.local/share/amtr   # or ~/.amtr, whichever it resolved to
```

Uninstalling with a snapshot still undelivered is safe: nothing reads the
marker once the hooks are gone.

## Files

- `.claude-plugin/plugin.json` — Claude Code manifest; names `hooks/claude.json`.
- `.codex-plugin/plugin.json` — Codex manifest; names `hooks/codex.json`, and
  adds `skills` so Codex finds `/amtr` (Claude Code takes `skills/` by
  convention).
- `hooks/claude.json`, `hooks/codex.json` — `PreCompact` capture +
  `UserPromptSubmit` delivery, declared per host.
- `tools/amtr-hook.sh` — all three hooks, dispatched on its first argument.
- `skills/amtr/SKILL.md` — the `/amtr <amtr_key> [clone]` wrapper.
