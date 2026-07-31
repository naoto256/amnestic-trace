# AMT Plugin

Amnestic Trace replaces a session's short-term working memory across a context
boundary. This plugin wires the two hooks that make that automatic on both
Claude Code and Codex, and ships the `/amtr` skill for handing memory to another
session.

## What it does

- **`PreCompact` hook** (declared in both `hooks/claude.json` and
  `hooks/codex.json` → `tools/amtr-hook.sh precompact`).
  Hands the journal path to `amtr synthesize`, which records an undelivered
  snapshot and detaches a worker before returning. Extraction therefore runs in
  parallel with compaction itself rather than delaying it.
- **`UserPromptSubmit` hook** (`tools/amtr-hook.sh recall`). This is where the
  post-compaction half actually happens: neither host can inject context from a
  compaction hook, so the snapshot is delivered at the next turn start. The
  hook waits while extraction is still running, gives up after 25s injecting
  nothing, and deletes the marker only once `amtr recall` reports that it
  actually printed a handoff — exit 0 means delivered, 1 means nothing was.
- **`/amtr` skill** (`skills/amtr/`). A thin wrapper over
  `amtr recall --amtr-key` for the cross-session case. Compaction
  inside one session needs no key and no skill.

Both hosts run the same `tools/amtr-hook.sh`, but each declares it in its own
file: `hooks/claude.json` and `hooks/codex.json`, each named by the `hooks` key
in that host's manifest. Neither is found by convention, so the pairing is
stated rather than inferred. The script is shared because the work is
identical; the declarations are split because what can be asserted about each
host is not.

Concretely, the Claude Code file sets `timeout` explicitly — 10s for the
capture hook, which returns as soon as the worker has detached, and 35s for the
delivery hook, which needs room for the 25s poll plus the read that follows.
The Codex file sets no timeout, because the unit of that field is not
documented for Codex and a wrong guess would kill the hook instantly rather
than fail visibly. Leaving it to the host's default is the honest default.

## Prerequisites

- `amtr` on `PATH`. Build and install it from the repo root:

  ```sh
  cargo install --path .
  ```

  That puts `amtr` in `~/.cargo/bin`. `~/.local/bin` works too; the hook script
  appends both, plus the Homebrew and `/usr/local` prefixes, because hook
  execution inherits a minimal `PATH` that omits them. Appended rather than
  prepended, so nothing here shadows the system's own tools.

  The name is `amtr` rather than `amt` because macOS ships an unrelated
  root-owned `/usr/sbin/amt` that wins on a default `PATH`.

- The host CLI that produced the journal (`claude` or `codex`) must be on
  `PATH` and authenticated — that is what performs the extraction. AMT reads
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
- **Codex**: launched with `--sandbox read-only`, which prevents writes. Codex
  offers no equivalent of "no tools", so **the agent keeps a shell and can read
  files you can read** — the sandbox confines writing, not reading, and the
  working directory sets where it starts rather than where it can reach.
  Outbound network is closed: measured under these flags, an HTTPS request to a
  hostname fails at name resolution (`curl` exits 6). A raw-address route was
  not tested, so read that as name resolution not working rather than as proof
  that nothing can leave.

So on Codex, a journal that successfully steers the extraction agent could have
it read something outside the working directory and fold that into the handoff,
which is injected into the next turn — where you would see it. If that matters
for your threat model, prefer the Claude Code path, or read `prompt.md` and keep
an eye on what lands in `prefrontal-cortex/`.

The agent runs in an empty temporary directory, deleted afterwards, so nothing
belonging to this tool — other sessions' handoffs, their keys, the prompt — sits
where it starts.

No daemon, no config file, and no environment variable. The extraction prompt
is materialized at `~/.local/share/amtr/prompt.md` on first run and is yours to
edit in place.

**Upgrades never touch it.** Once that file exists it is treated as yours, so a
newer version's default prompt is not written over it — an upgrade cannot
silently replace prompt text you tuned. The cost is that improvements to the
default do not reach an existing install either. To take a new default, delete
the file and let the next run write it:

```sh
rm ~/.local/share/amtr/prompt.md
```

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

`codex exec` does fire the delivery hook — it emits `UserPromptSubmit` like an
interactive session. It simply has nothing to deliver: a non-interactive run is
its own session with its own id, so the hook finds no marker for it and exits
without injecting. The same is true of the extraction subprocess this tool
launches, which is why that does not feed itself its own memory.

## Uninstall

```sh
claude plugin uninstall amtr@naoto256-amtr
codex plugin remove amtr@naoto256-amtr
```

Removing the plugin stops all capture and injection but leaves stored memory in
place. To discard that too:

```sh
rm -rf ~/.local/share/amtr   # or ~/.amtr if ~/.local is absent
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
- `tools/amtr-hook.sh` — both hooks, dispatched on its first argument.
- `skills/amtr/SKILL.md` — the `/amtr <amtr_key> [clone]` wrapper.
