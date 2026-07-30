# AMT Plugin

Amnestic Trace replaces a session's short-term working memory across a context
boundary. This plugin wires the two hooks that make that automatic on both
Claude Code and Codex, and ships the `/amtr` skill for handing memory to another
session.

## What it does

- **`PreCompact` hook** (`hooks/claude.json` → `tools/amtr-hook.sh precompact`).
  Hands the journal path to `amtr synthesize`, which records an undelivered
  snapshot and detaches a worker before returning. Extraction therefore runs in
  parallel with compaction itself rather than delaying it.
- **`UserPromptSubmit` hook** (`tools/amtr-hook.sh recall`). This is where the
  post-compaction half actually happens: neither host can inject context from a
  compaction hook, so the snapshot is delivered at the next turn start. The
  hook waits while extraction is still running, gives up after 25s injecting
  nothing, and deletes the marker only once the text has been injected.
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

  That puts `amtr` in `~/.cargo/bin`. `~/.local/bin` works too; the
  hook script prepends both, plus the Homebrew and `/usr/local` prefixes,
  because hook execution inherits a minimal `PATH` that omits them.

  The name is `amtr` rather than `amt` because macOS ships an unrelated
  root-owned `/usr/sbin/amt` that wins on a default `PATH`.

- The host CLI that produced the journal (`claude` or `codex`) must be on
  `PATH` and authenticated — that is what performs the extraction. AMT reads
  the journal to decide which one to launch and never holds credentials itself.

No daemon, no config file, and no environment variable. The extraction prompt
is materialized at `~/.local/share/amtr/prompt.md` on first run and is yours to
edit in place.

## Install

Both hosts discover plugins through marketplace catalogs, not by scanning
directories. The repo root carries a `.claude-plugin/marketplace.json` that
points at this `plugin/` subdirectory as the install source, and it serves both
hosts.

### Claude Code

```sh
claude plugin marketplace add naoto256/amnestictrace
claude plugin install amtr@naoto256-amtr
```

From a local checkout during development:

```sh
claude plugin marketplace add /absolute/path/to/amnestictrace
claude plugin install amtr@naoto256-amtr
```

### Codex

```sh
codex plugin marketplace add naoto256/amnestictrace
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
codex_hooks = true
```

Restart the session on either host so the hooks take effect.

Codex installs a **copy** into `~/.codex/plugins/cache/`, so editing this
directory does not change what runs until the marketplace snapshot is
refreshed (`codex plugin marketplace upgrade naoto256-amtr`).

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
