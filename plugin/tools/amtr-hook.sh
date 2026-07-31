#!/bin/sh
# Amnestic Trace hook entry point for both Claude Code and Codex.
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

# `set -u` would abort on an unset HOME, and this must never be the reason a
# turn fails.
home=${HOME:-}
[ -n "$home" ] || exit 0

# Minimal JSON string field read; jq and python3 are not assumed. The FIRST
# match, and not because of field order — nothing guarantees that. A JSON string
# cannot hold a raw `"`, so the user's prompt text, which travels in this same
# payload, can only contain an ESCAPED `\"session_id\": \"...\"`; requiring an
# unescaped opening quote means a prompt cannot outrank the host's own field.
field() {
	printf '%s' "$input" |
		grep -o "\"$1\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" |
		head -1 |
		sed "s/^\"$1\"[[:space:]]*:[[:space:]]*\"//; s/\"$//"
}

session_id=$(field session_id)
[ -n "$session_id" ] || exit 0

# Everything downstream interpolates this into a path and into a `find` pattern.
# Both hosts mint UUIDs, so anything outside this set is not a session id we
# recognise, and guessing at its intent is worse than doing nothing.
case "$session_id" in
*[!A-Za-z0-9._-]*) exit 0 ;;
esac

# Same rule the binary uses, and for the same reason: a hook has no guaranteed
# shell environment, so the layout must not depend on a tunable. An existing
# `~/.amtr` wins, because this is evaluated at every start and a `~/.local` that
# appears later must not move the store away from rows already written there. A
# test in src/store.rs lifts these lines out and runs them against the binary's
# rule, so editing one side alone fails `cargo test`.
if [ -d "$home/.amtr" ]; then
	amtr_home="$home/.amtr"
elif [ -d "$home/.local" ]; then
	amtr_home="$home/.local/share/amtr"
else
	amtr_home="$home/.amtr"
fi

# Must match `slug()` in src/store.rs exactly, or the reader looks for a marker
# at a path the writer never wrote. All three of the binary's steps are here,
# including the substitution the gate above makes unreachable: resting the
# agreement on that gate would let a later edit loosening it desynchronise the
# two. A test in src/store.rs lifts these two lines out and runs them against
# the binary's rule, so editing one side alone fails `cargo test`.
#
# LC_ALL=C pins sed to bytes, which is what the binary substitutes in; under a
# UTF-8 locale some seds take a multibyte character as one unit instead.
slug=$(printf '%s' "$session_id" | LC_ALL=C sed 's/[^A-Za-z0-9._-]/_/g; s/^\.*//; s/\.*$//')
[ -n "$slug" ] || slug=_
marker="$amtr_home/prefrontal-cortex/$slug.marker"

# Hook execution inherits a minimal PATH that omits where cargo and the usual
# installers write, so `command -v` would otherwise miss an installed binary.
# Appended, not prepended: this process goes on to run stock utilities, and a
# hook has no business shadowing the system's copies.
PATH="$PATH:$home/.local/bin:$home/.cargo/bin:/opt/homebrew/bin:/usr/local/bin"
export PATH

command -v amtr >/dev/null 2>&1 || exit 0

# After the binary is known to exist, so a machine with the plugin but no binary
# does not grow a store on every turn — but before the redirect below, since the
# shell opens that first and skips the whole command if it cannot.
#
# The parent goes at the ambient umask: it is `~/.local/share` or similar,
# shared with other tools and not ours to tighten. Only the store itself goes to
# 0700, in a subshell so the umask does not leak.
mkdir -p "$(dirname "$amtr_home")" 2>/dev/null
(
	umask 077
	mkdir -p "$amtr_home" 2>/dev/null
	# `true`, not `:` — see the note below on special built-ins.
	[ -e "$amtr_home/amtr.log" ] || true >>"$amtr_home/amtr.log" 2>/dev/null
	# `log_stderr_to`'s O_CREAT mode applies only to a file it creates, so a log
	# left readable by an older install keeps what it had.
	chmod 600 "$amtr_home/amtr.log" 2>/dev/null
) 2>/dev/null

# Probed once here rather than left inline below, because a shell skips any
# command whose redirect will not open — so inline, an unwritable log would
# decide whether the binary runs at all. The binary points its own stderr here
# as soon as it starts, so the fallback costs a few lines, not the tool.
#
# `true`, not `:`. A redirection error on a POSIX *special* built-in terminates
# the shell, and `:` is one, so the obvious `{ : >>"$log"; } || log=/dev/null`
# never reaches its fallback: dash exits 2 on the spot. bash and zsh are
# lenient; `/bin/sh` is dash on Debian and Ubuntu.
log="$amtr_home/amtr.log"
{ true >>"$log"; } 2>/dev/null || log=/dev/null

case "$event" in
precompact)
	journal=$(field transcript_path)
	[ -n "$journal" ] || journal=$(field rollout_path)
	if [ -z "$journal" ]; then
		# DESIGN-QUESTION: Codex's PreCompact payload is not documented. Its
		# binary carries the string `rollout_path` (not `transcript_path`), but
		# whether PreCompact actually delivers it is unverified, so this falls
		# back to the rollout filename, which embeds the session id. The id is
		# validated above, so it holds no `find` glob metacharacters.
		journal=$(find "$home/.codex/sessions" -name "*$session_id*.jsonl" 2>/dev/null | head -1)
	fi
	[ -n "$journal" ] && [ -f "$journal" ] || exit 0
	# Returns as soon as the worker has detached and the marker is on disk.
	# stderr goes to the log because the binary cannot redirect its own until it
	# has resolved the home directory, and a home that will not resolve is
	# exactly the failure that would leave no trace.
	amtr synthesize "$session_id" "$journal" >/dev/null 2>>"$log"
	;;
recall)
	# The marker is an undelivered snapshot, not a "compaction happened" flag.
	# Its absence is the normal state of every turn not following a compaction.
	[ -f "$marker" ] || exit 0

	# The poll budget is 25s. hooks/claude.json declares a 35s timeout around
	# it, leaving room for the read that follows; hooks/codex.json declares
	# none, because the unit of that field is unverified on Codex and a wrong
	# guess would kill the hook outright rather than fail visibly.
	waited=0
	while [ "$waited" -lt 25 ]; do
		case "$(cat "$marker" 2>/dev/null)" in
		ongoing) ;;
		*) break ;;
		esac
		sleep 1
		waited=$((waited + 1))
	done

	# The marker names the snapshot it owes: `ready:<amtr_key>`. Captured whole,
	# so the debt discharged at the end is provably the one delivered here.
	claim=$(cat "$marker" 2>/dev/null)
	case "$claim" in
	ready:*) ;;
	*)
		# Fail open: still extracting, or the worker gave up. The transcript
		# survives, so the damage is that this compaction falls back to the
		# host's native summary. Dropping the marker here is safe — a worker
		# that lands late rewrites it and the next turn delivers.
		rm -f "$marker"
		exit 0
		;;
	esac

	# Deliver first, then discharge, so nothing is marked delivered on a turn
	# that failed to inject it: `amtr recall` exits 0 only when it printed a
	# handoff. stderr goes to the log because an unparseable row fails here on
	# every turn and the reason is the only evidence of it.
	if amtr recall "$session_id" 2>>"$log"; then
		# The whole claim, not just its shape. A compaction can finish while
		# this turn is in flight, and its `ready:<newer key>` must not be
		# discharged by the turn that delivered the older one.
		if [ "$(cat "$marker" 2>/dev/null)" = "$claim" ]; then
			rm -f "$marker"
		fi
	fi
	;;
esac
exit 0
