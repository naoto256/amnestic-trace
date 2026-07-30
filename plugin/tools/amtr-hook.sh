#!/bin/sh
# AMT hook entry point for both Claude Code and Codex.
#
#   amtr-hook.sh precompact   <- PreCompact:        start a detached synthesize
#   amtr-hook.sh recall       <- UserPromptSubmit:  inject the replacement memory
#
# PreCompact/PostCompact hooks cannot inject context on either host, so the
# post-compaction half is realized at the next turn start.
#
# Every path exits 0 with no stdout unless there is something to inject: the
# hook must never be the reason a turn fails.
set -u

event=${1:-}
input=$(cat)

# Minimal JSON string field read. The hook payloads are flat, and depending on
# jq or python3 being installed would make the hook fail where AMT does not.
#
# `grep -o` takes the FIRST occurrence rather than sed's greedy `.*` prefix,
# which would take the last. That matters: the user's own prompt text is in
# this payload, so a prompt containing `"session_id":"<other>"` could otherwise
# redirect the lookup and inject a different session's memory. The host's real
# fields precede the prompt in the payload.
field() {
	printf '%s' "$input" |
		grep -o "\"$1\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" |
		head -1 |
		sed "s/^\"$1\"[[:space:]]*:[[:space:]]*\"//; s/\"$//"
}

session_id=$(field session_id)
[ -n "$session_id" ] || exit 0

# Same two-way branch the binary uses, and for the same reason: a hook has no
# guaranteed shell environment, so the layout must not depend on one.
if [ -d "$HOME/.local" ]; then
	amtr_home="$HOME/.local/share/amtr"
else
	amtr_home="$HOME/.amtr"
fi
slug=$(printf '%s' "$session_id" | tr -c 'A-Za-z0-9._-' '_')
marker="$amtr_home/prefrontal-cortex/$slug.marker"

# Hook execution inherits a minimal PATH that omits the directories cargo and
# the usual installers write to, so `command -v` would miss an installed binary
# and this script would silently do nothing.
PATH="$HOME/.local/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
export PATH

command -v amtr >/dev/null 2>&1 || exit 0

case "$event" in
precompact)
	journal=$(field transcript_path)
	[ -n "$journal" ] || journal=$(field rollout_path)
	if [ -z "$journal" ]; then
		# DESIGN-QUESTION: Codex's PreCompact payload is not documented. Its
		# binary carries the string `rollout_path` (not `transcript_path`), but
		# whether PreCompact actually delivers it is unverified, so this falls
		# back to the rollout filename, which embeds the session id.
		journal=$(find "$HOME/.codex/sessions" -name "*$session_id*.jsonl" 2>/dev/null | head -1)
	fi
	[ -n "$journal" ] && [ -f "$journal" ] || exit 0
	# Returns as soon as the worker has detached and the marker is on disk.
	amtr synthesize "$session_id" "$journal" >/dev/null 2>&1
	;;
recall)
	# The marker is an undelivered snapshot, not a "compaction happened" flag.
	# Its absence means there is nothing owed, which is the normal state of
	# every turn that does not follow a compaction.
	[ -f "$marker" ] || exit 0

	# DESIGN-QUESTION: the poll budget is 25s because the UserPromptSubmit hook
	# default timeout is 30s, and the `timeout` key's unit (seconds or
	# milliseconds) is documented inconsistently — so hooks.json declares none
	# rather than risk a value that kills the hook instantly.
	waited=0
	while [ "$(cat "$marker" 2>/dev/null)" = "ongoing" ] && [ "$waited" -lt 25 ]; do
		sleep 1
		waited=$((waited + 1))
	done

	if [ "$(cat "$marker" 2>/dev/null)" != "ready" ]; then
		# Fail open: still extracting, or the worker gave up. The transcript
		# survives, so the damage is that this compaction falls back to the
		# host's native summary. Dropping the marker here is safe — a worker
		# that lands late rewrites it to `ready` and the next turn delivers.
		rm -f "$marker"
		exit 0
	fi

	# Deliver first, then discharge the debt, so a snapshot is never marked
	# delivered on a turn that failed to inject it.
	if amtr recall "$session_id" 2>/dev/null; then
		rm -f "$marker"
	fi
	;;
esac
exit 0
