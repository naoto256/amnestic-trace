# AMT Plugin

Amnestic Trace replaces a session's short-term working memory across a context
boundary. This plugin wires the two hooks that make that automatic on both
Claude Code and Codex, and ships the `/amt` skill for handing memory to another
session.

## What it does

- **`PreCompact` hook** (`hooks/hooks.json` → `tools/amt-hook.sh precompact`).
  Hands the journal path to `amt synthesize`, which records an undelivered
  snapshot and detaches a worker before returning. Extraction therefore runs in
  parallel with compaction itself rather than delaying it.
- **`UserPromptSubmit` hook** (`tools/amt-hook.sh recall`). This is where the
  post-compaction half actually happens: neither host can inject context from a
  compaction hook, so the snapshot is delivered at the next turn start. The
  hook waits while extraction is still running, gives up after 25s injecting
  nothing, and deletes the marker only once the text has been injected.
- **`/amt` skill** (`skills/amt/`). A thin wrapper over
  `amt recall --amt-key` for the cross-session case. Compaction inside one
  session needs no key and no skill.

Both hosts run the same `tools/amt-hook.sh`; the
`${PLUGIN_ROOT:-$CLAUDE_PLUGIN_ROOT}` expansion in `hooks/hooks.json` resolves
either host's plugin-path variable.

## Prerequisites

- `amt` on `PATH`. Build and install it from the repo root:

  ```sh
  cargo install --path .
  ```

  That puts `amt` in `~/.cargo/bin`. `~/.local/bin` works too; the hook script
  prepends both, plus the Homebrew and `/usr/local` prefixes, because hook
  execution inherits a minimal `PATH` that omits them.

  **Name collision.** macOS ships an unrelated root-owned `/usr/sbin/amt`, and
  on a default `PATH` it wins — `command -v amt` will point at the system
  binary, not this one. The hooks and the `/amt` skill both prepend the install
  directories so they always reach the right one, but if you invoke `amt` by
  hand, check `command -v amt` first.

- The host CLI that produced the journal (`claude` or `codex`) must be on
  `PATH` and authenticated — that is what performs the extraction. AMT reads
  the journal to decide which one to launch and never holds credentials itself.

No daemon, no config file, and no environment variable. The extraction prompt
is materialized at `~/.local/share/amt/prompt.md` on first run and is yours to
edit in place.

## Install

Both hosts discover plugins through marketplace catalogs, not by scanning
directories. The repo root carries a `.claude-plugin/marketplace.json` that
points at this `plugin/` subdirectory as the install source, and it serves both
hosts.

### Claude Code

```sh
claude plugin marketplace add naoto256/amnestictrace
claude plugin install amt@naoto256-amt
```

From a local checkout during development:

```sh
claude plugin marketplace add /absolute/path/to/amnestictrace
claude plugin install amt@naoto256-amt
```

### Codex

```sh
codex plugin marketplace add naoto256/amnestictrace
codex plugin add amt@naoto256-amt
```

From a local checkout:

```sh
codex plugin marketplace add /absolute/path/to/amnestictrace
codex plugin add amt@naoto256-amt
```

Codex reads `.codex-plugin/plugin.json` and `hooks/hooks.json` from this same
directory. Hooks must also be enabled in `~/.codex/config.toml`:

```toml
[features]
codex_hooks = true
```

Restart the session on either host so the hooks take effect.

Codex installs a **copy** into `~/.codex/plugins/cache/`, so editing this
directory does not change what runs until the marketplace snapshot is
refreshed (`codex plugin marketplace upgrade naoto256-amt`).

## Uninstall

```sh
claude plugin uninstall amt@naoto256-amt
codex plugin remove amt@naoto256-amt
```

Removing the plugin stops all capture and injection but leaves stored memory in
place. To discard that too:

```sh
rm -rf ~/.local/share/amt      # or ~/.amt if ~/.local does not exist
```

Uninstalling with a snapshot still undelivered is safe: nothing reads the
marker once the hooks are gone.

## Files

- `.claude-plugin/plugin.json` — Claude Code manifest.
- `.codex-plugin/plugin.json` — Codex manifest; adds `skills` so Codex finds
  `/amt` (Claude Code discovers `skills/` by convention).
- `hooks/hooks.json` — `PreCompact` capture + `UserPromptSubmit` delivery.
- `tools/amt-hook.sh` — both hooks, dispatched on its first argument.
- `skills/amt/SKILL.md` — the `/amt <amt_key> [clone]` wrapper.
