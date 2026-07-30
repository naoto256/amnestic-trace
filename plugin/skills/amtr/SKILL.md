---
name: amtr
description: Take over another session's working memory by its AMTR key. Use when the user says "/amtr <amtr_key>", "/amtr <amtr_key> clone", or otherwise asks to pick up, inherit, or continue from a snapshot named like amtr-3k9f2x1.
---

# /amtr — adopt a working-memory snapshot

Thin wrapper over `amtr recall --amtr-key`. Compaction inside one session needs no
key and no skill; this is only for the cross-session case, where the key must be
typed because there is no other channel between two sessions.

## Arguments

```
/amtr <amtr_key>          take over the snapshot (MOVE — the giving session forgets)
/amtr <amtr_key> clone    copy it instead (the giving session keeps its memory)
```

The default is a handoff (引き継ぎ), not a fan-out: after a move, the giving
session's next compaction starts from nothing. Use `clone` only when the other
session is meant to keep working.

## Steps

<!--
Neither variable is in either host's public docs; both were confirmed by env
dump on a real machine. CODEX_THREAD_ID matches the UUID in the session's own
rollout filename, which is the id AMTR keys rows by.
-->

1. Run exactly one command, using whichever session variable is set:

   ```sh
   PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH" \
     amtr recall "${CLAUDE_CODE_SESSION_ID:-$CODEX_THREAD_ID}" --amtr-key <amtr_key>
   ```

   Add `--clone` when the user asked for `clone`.

   The `PATH` prefix is there because the shell you get may not include the
   directory the binary was installed into.

2. Adopt what the command prints as your own working memory for this session,
   and continue the user's work from it. It is a replacement, not a reference:
   treat it as what you already knew.

3. Report the AMTR key from the trailing line to the user verbatim. That line is
   the only way the human learns the current key, so it must not be paraphrased
   away.

## When it prints nothing

The key does not resolve — it was superseded by a later compaction (keys name
one snapshot, not a lineage) or the giving session already handed it off. Say
so and continue without it. Do not retry and do not guess another key.
