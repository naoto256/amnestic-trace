#!/bin/sh
# Amnestic Trace hook entry point for both Claude Code and Codex.
#
#   amtr-hook.sh precompact   <- PreCompact:        start a detached synthesize
#   amtr-hook.sh deliver      <- PreToolUse:        inject at the first tool call
#   amtr-hook.sh recall       <- UserPromptSubmit:  inject at the next turn start
#
# PreCompact/PostCompact hooks cannot inject context on either host, so the
# post-compaction half is realized later, and "later" is the whole problem this
# has two deliverers for. A compaction fires mid-turn; the session then keeps
# working, sometimes for half an hour, and `UserPromptSubmit` does not run again
# until the user says something. Everything done in between is missing from a
# memory that was accurate when it was taken. `PreToolUse` runs throughout that
# stretch, so it delivers first and the turn-start hook becomes the backstop for
# turns that call no tools at all.
#
# Whichever arrives first discharges the marker, and the marker is what keeps
# the other from delivering it twice. Both will wait up to 25s for an
# extraction still in flight, but they spend that budget differently: the
# turn-start hook has one chance per turn and spends it on every turn it runs,
# while the tool-call hook spends one budget per compaction, shared by every
# tool call in the stretch that follows it.
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
# Holds the epoch second at which the tool-call deliverer stops waiting for the
# snapshot this marker owes. See the `deliver` case for why the budget is spent
# once per debt rather than once per tool call.
deadline_file="$amtr_home/prefrontal-cortex/$slug.deliver-deadline"

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

# Hands the memory to the host, then discharges the debt — in that order, so
# nothing is marked delivered by a hook that failed to inject it. `amtr recall`
# exits 0 only when it printed a handoff.
#
# `$1` is the event whose shape the output must take. The binary builds that
# JSON rather than this script: a handoff is machine-written from a journal and
# arrives full of quotes, backslashes and newlines, and a shell wrapping JSON
# around that by hand is one unescaped byte away from delivering nothing at all.
#
# stderr goes to the log because an unparseable row fails here on every turn,
# and the reason is the only evidence anyone gets.
deliver_claim() {
	# Take the marker before injecting anything, by renaming it somewhere only
	# this process knows. Concurrent tool calls can reach here at the same
	# instant — the tool-call deliverer wakes every waiter on one deadline, so
	# they arrive together by design rather than by coincidence — and a rename
	# is the one operation exactly one of them can win. Whoever loses finds no
	# marker to move and steps aside, which is what stops the same memory being
	# injected twice into the same turn.
	#
	# Discharging afterwards cannot do this job: by the time the marker is
	# removed the handoff has already gone to the host.
	pending="$marker.delivering.$$"
	mv "$marker" "$pending" 2>/dev/null || return 0

	# Identity is checked here rather than after delivery, because holding the
	# marker is what makes the answer stable. A compaction landing between the
	# read that produced `$2` and the rename above leaves a newer claim in
	# hand, and delivering that one would hand over a snapshot nobody asked
	# for and discharge a debt nobody paid.
	if [ "$(cat "$pending" 2>/dev/null)" != "$2" ]; then
		restore_claim
		return 0
	fi

	# `--expect` names the snapshot the marker owed, so the binary can decline
	# a row that is no longer the one claimed here.
	if amtr recall "$session_id" --hook-json "$1" --expect "${2#ready:}" 2>>"$log"; then
		rm -f "$pending" "$deadline_file"
	else
		# Nothing reached the host, so the debt still stands.
		restore_claim
	fi
}

# Puts a claim back after a delivery that did not happen. `set -C` so a newer
# compaction that wrote its own marker while this one was held is not overwritten
# by the older claim being returned; in that case the older one is simply
# dropped, which is the same thing that happens to any superseded snapshot.
#
# Nothing is restored from a claim file that is missing or empty. The redirect
# below opens `$marker` before `cat` reads, so a `cat` with nothing to read
# would leave an empty marker standing in for a real one — a claim shaped like
# a debt that names no snapshot.
restore_claim() {
	if [ -s "$pending" ]; then
		(set -C; cat "$pending" >"$marker") 2>/dev/null
	fi
	rm -f "$pending"
}

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
	# A new compaction is a new debt, and a new debt gets its own waiting
	# budget. Cleared before the marker is written so a tool call landing
	# between the two cannot inherit the previous debt's spent deadline.
	rm -f "$deadline_file"
	# A claim held for delivery is normally either discharged or restored, but
	# a hook killed between the two leaves the file it was holding behind, and
	# nothing else would ever remove it. Swept here because a new compaction is
	# the point at which any claim still outstanding is superseded anyway: what
	# these files hold is a snapshot this compaction is about to replace. An
	# unmatched glob is passed through literally, which `rm -f` ignores.
	rm -f "$marker".delivering.*
	# Returns as soon as the worker has detached and the marker is on disk.
	# stderr goes to the log because the binary cannot redirect its own until it
	# has resolved the home directory, and a home that will not resolve is
	# exactly the failure that would leave no trace.
	amtr synthesize "$session_id" "$journal" >/dev/null 2>>"$log"
	;;
deliver)
	# PreToolUse. Runs many times a turn, so it does nothing but look at one
	# file unless a snapshot is actually owed.
	[ -f "$marker" ] || exit 0

	claim=$(cat "$marker" 2>/dev/null)
	case "$claim" in
	ready:*) ;;
	ongoing)
		# The budget is 25s per DEBT, not per tool call, and it is a wall-clock
		# deadline rather than a per-process countdown so that concurrent tool
		# calls share one window instead of each opening its own.
		#
		# Waiting at all is the point: this hook exists because a session that
		# keeps working after a compaction does so without its memory, and every
		# tool call made in that stretch is one made blind. Waiting once buys the
		# most dangerous of them — the first few, before the extraction lands —
		# a chance to arrive informed.
		#
		# Waiting only once is the other half. Polling on every tool call would
		# stall the session's real work for as long as the extraction takes, and
		# an extraction that has died would stall it forever; the cost of being
		# wrong is unbounded where the benefit is not. After the deadline this
		# steps aside as before, because a later tool call is moments away and
		# the turn-start hook is still behind it.
		#
		# So this does not eliminate memory-less tool calls. It bounds them: the
		# unbounded "until the user next speaks" becomes "until the extraction
		# finishes or 25s, whichever is sooner".
		now=$(date +%s 2>/dev/null) || exit 0
		case "$now" in
		'' | *[!0-9]*) exit 0 ;;
		esac
		# One name for the budget, so the value that opens a window and the
		# value that bounds a window cannot drift apart.
		budget=25
		# `set -C` in a subshell so the flag does not leak: whoever creates the
		# file names the deadline, everyone else reads it. The value is written
		# rather than inferred from mtime because `stat` spells that differently
		# on macOS and Linux, and this runs on both under four shells.
		if (set -C; printf '%s' "$((now + budget))" >"$deadline_file") 2>/dev/null; then
			deadline=$((now + budget))
		else
			deadline=$(cat "$deadline_file" 2>/dev/null)
			case "$deadline" in
			# Unreadable or half-written: treat the budget as spent. Guessing a
			# fresh one here is how one debt's window becomes many.
			'' | *[!0-9]*) exit 0 ;;
			esac
			# Every deadline is some earlier reader's `now + budget`, and this
			# reader's clock is at or past that reader's, so a deadline can
			# never be more than one budget ahead of it. Further ahead than that
			# means the clock has jumped backwards under us, or the file did not
			# come from this code. Treat it as spent rather than sit in it: the
			# host's hook timeout would eventually end the wait, but a bound
			# this script can derive from its own invariant is a bound it should
			# not be borrowing from the host.
			[ "$((deadline - now))" -le "$budget" ] || exit 0
		fi

		while [ "$now" -lt "$deadline" ]; do
			case "$(cat "$marker" 2>/dev/null)" in
			ongoing) ;;
			*) break ;;
			esac
			sleep 1
			now=$(date +%s 2>/dev/null) || break
			case "$now" in
			'' | *[!0-9]*) break ;;
			esac
		done

		claim=$(cat "$marker" 2>/dev/null)
		case "$claim" in
		ready:*) ;;
		# Still not ready, so nothing to deliver — and, unlike the turn-start
		# path, the marker stays. Giving up on the debt is the backstop's
		# decision to make; this one is only ever early.
		*) exit 0 ;;
		esac
		;;
	*) exit 0 ;;
	esac

	deliver_claim PreToolUse "$claim"
	;;
recall)
	# The marker is an undelivered snapshot, not a "compaction happened" flag.
	# Its absence is the normal state of every turn not following a compaction —
	# including every turn where the delivery above already discharged it.
	[ -f "$marker" ] || exit 0

	# The poll budget is 25s, spent per turn: this hook runs once a turn and a
	# turn that misses the memory runs without it entirely. Contrast the
	# tool-call deliverer above, which spends 25s per compaction across all the
	# tool calls that follow it. hooks/claude.json declares a 35s timeout around
	# both, leaving room for the read that follows; hooks/codex.json declares
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
		rm -f "$marker" "$deadline_file"
		exit 0
		;;
	esac

	deliver_claim UserPromptSubmit "$claim"
	;;
esac
exit 0
