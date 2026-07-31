---
name: amtr
description: Take over another session's working memory by its AMTR key. Use when the user says "/amtr <amtr_key>", "/amtr <amtr_key> clone", or otherwise asks to pick up, inherit, or continue from a snapshot named like amtr-ms7uix6i-3f9k2xq1.
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

1. Check the key first. A valid one looks like `amtr-` followed by two
   base36 groups separated by a hyphen — `amtr-ms7uix6i-3f9k2xq1`. It contains
   only lowercase letters, digits and hyphens. If what the user typed does not
   match that, stop and ask them to re-read it rather than passing it through;
   it is a key they read off another session's output, so a transcription slip
   is the likely explanation.

2. Run exactly one command, using whichever session variable is set:

   ```sh
   PATH="$PATH:$HOME/.local/bin:$HOME/.cargo/bin" \
     amtr recall "${CLAUDE_CODE_SESSION_ID:-$CODEX_THREAD_ID}" --amtr-key "<amtr_key>"
   ```

   Substitute the key inside the quotes and keep them. Add `--clone` when the
   user asked for `clone`.

   The `PATH` suffix is there because the shell you get may not include the
   directory the binary was installed into. It is appended, not prepended, so
   nothing here shadows a system tool.

3. Adopt what the command prints as your own working memory for this session,
   and continue the user's work from it. It is a replacement, not a reference:
   treat it as what you already knew.

4. Report the AMTR key from the trailing line to the user verbatim. That line is
   the only way the human learns the current key, so it must not be paraphrased
   away.

   A clone prints no key line, because a clone has no key until the next
   compaction mints one. When there is no line, there is nothing to report —
   say nothing about keys rather than explaining their absence.

## When it prints nothing

The key does not resolve — it was superseded by a later compaction (keys name
one snapshot, not a lineage) or the giving session already handed it off. Say
so and continue without it. Do not retry and do not guess another key.
