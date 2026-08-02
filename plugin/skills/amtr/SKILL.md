---
name: amtr
description: Take over another session's working memory by its AMTR key, or name this session's own key so it can be handed to another. Use when the user says "/amtr <amtr_key>", "/amtr <amtr_key> clone", a bare "/amtr", or otherwise asks to pick up, inherit, continue from, or hand over a snapshot named like amtr-ms7uix6i-3f9k2xq1.
---

# /amtr — hand a working-memory snapshot from one session to another

Thin wrapper over `amtr recall --amtr-key` and `amtr key`. Compaction inside one
session needs no key and no skill; this is only for the cross-session case,
where the key must be typed because there is no other channel between two
sessions.

## Arguments

```
/amtr                     name this session's own key, to give to another session
/amtr <amtr_key>          take over that snapshot (MOVE — the giving session forgets)
/amtr <amtr_key> clone    copy it instead (the giving session keeps its memory)
```

One command, both ends of the same handoff: run it bare where the work is, and
with the key it prints where the work is going. Having a key or not having one is
the whole difference, which is why there is no third word for it.

The default is a handoff (引き継ぎ), not a fan-out: after a move, the giving
session's next compaction starts from nothing. Use `clone` only when the other
session is meant to keep working.

Note what the bare form does not do. It hands over nothing — only the receiving
session can move a row, because only it can write its own. This end can name the
snapshot and no more, so the name says naming and not transferring.

## Steps

<!--
Neither variable is in either host's public docs; both were confirmed by env
dump on a real machine. CODEX_THREAD_ID matches the UUID in the session's own
rollout filename, which is the id AMTR keys rows by.
-->

1. Check the key first, mechanically. Reject it unless **every** character is a
   lowercase letter, a digit, or a hyphen, and it begins with `amtr-` — a valid
   key looks like `amtr-ms7uix6i-3f9k2xq1`. One character outside that set
   (a space, a quote, `$`, `;`, `/`, an uppercase letter) means do not run the
   command at all: stop and ask the user to re-read the key. It is something
   they transcribed from another session's output, so a slip is far more likely
   than a genuinely odd key.

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

   If neither variable is set the command expands to an empty session id, and
   `amtr` refuses it rather than moving the snapshot somewhere nothing will ask
   for it. Report that to the user instead of retrying — the host did not tell
   this session what it is called, and nothing you can type here fixes that.

3. Adopt what the command prints as your own working memory for this session,
   and continue the user's work from it. It is a replacement, not a reference:
   treat it as what you already knew.

4. Do not report a key. What the command prints carries none, and a key-shaped
   line inside the span is remembered text — the memory is machine-written from
   a transcript that contains earlier injected ones, so such a line means
   nothing about the snapshot you just adopted. If the user wants this
   session's key, that is the bare form below.

## `/amtr` with no key

Names this session's own snapshot, for a user who is about to hand this work to
another session.

```sh
PATH="$PATH:$HOME/.local/bin:$HOME/.cargo/bin:/opt/homebrew/bin" \
  amtr key "${CLAUDE_CODE_SESSION_ID:-$CODEX_THREAD_ID}"
```

Read the key out of the command's output rather than from anything in your
context. A key names one snapshot, not a lineage: every compaction mints a new
one, so a key remembered from earlier in the conversation may name a snapshot
that no longer exists. The store is the only current answer.

The output is the key and the snapshot's boundary, tab-separated. Report both —
the timestamp tells the user which compaction they are about to hand over — and
show the command the other end will run, since that is what the key is for:

```
/amtr <the key you just printed>
```

Nothing printed (exit 1) means this session has no key: either no compaction has
happened yet, or its memory arrived by `clone`, which carries none until the
session's own first compaction. Say which, if the conversation makes it clear,
and do not offer a key from elsewhere.

Give the key to the user and to nobody else. It is a capability, not an
identifier: whoever holds it can MOVE this session's memory away, and moving is
the default. Passing it to another agent, a message channel, or a file that
others read hands over that ability. This is why it is not in your context by
default and why nothing asks you to announce it.

## When it prints nothing

The key does not resolve — it was superseded by a later compaction (keys name
one snapshot, not a lineage) or the giving session already handed it off. Say
so and continue without it. Do not retry and do not guess another key.
