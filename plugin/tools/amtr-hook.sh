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

# `set -u` would abort on an unset HOME, and this must never be the reason a
# turn fails.
home=${HOME:-}
[ -n "$home" ] || exit 0

# Minimal JSON string field read. The payloads are flat, and depending on jq or
# python3 being installed would make the hook fail where the binary does not.
#
# This takes the FIRST match rather than the last. The reason is not field
# order — nothing guarantees that. It is that a JSON string cannot contain a raw
# `"`, so the user's prompt text, which travels in this same payload, can only
# ever contain an ESCAPED `\"session_id\": \"...\"`. The pattern requires an
# unescaped opening quote, so a prompt cannot forge a match that outranks the
# host's own field.
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

# Same two-way branch the binary uses, and for the same reason: a hook has no
# guaranteed shell environment, so the layout must not depend on a tunable.
if [ -d "$home/.local" ]; then
	amtr_home="$home/.local/share/amtr"
else
	amtr_home="$home/.amtr"
fi

# Must match `slug()` in src/store.rs exactly, or the reader looks for a marker
# at a path the writer never wrote. All three of the binary's steps are here:
# replace anything outside the allowed class, trim leading and trailing dots,
# fall back to `_` when nothing survives.
#
# The character-class gate above already rejects ids that would need the first
# step, which made it look redundant. It is not: relying on the gate leaves the
# agreement resting on a check somewhere else in the file, and a later edit
# loosening that check would silently desynchronise the two implementations.
#
# A test in src/store.rs lifts these two lines out of this file and runs them
# against the binary's rule, so changing them here without changing there fails
# `cargo test`.
#
# LC_ALL=C pins sed to bytes. Under a UTF-8 locale some seds treat a multibyte
# character as one unit and others as its bytes, and the binary substitutes per
# byte — so without this the two agree or disagree depending on the environment
# the host happened to hand the hook.
slug=$(printf '%s' "$session_id" | LC_ALL=C sed 's/[^A-Za-z0-9._-]/_/g; s/^\.*//; s/\.*$//')
[ -n "$slug" ] || slug=_
marker="$amtr_home/prefrontal-cortex/$slug.marker"

# Hook execution inherits a minimal PATH that omits where cargo and the usual
# installers write, so `command -v` would otherwise miss an installed binary and
# this script would silently do nothing. Appended rather than prepended: this
# process goes on to run stock utilities, and a hook has no business shadowing
# the system's copies of them.
PATH="$PATH:$home/.local/bin:$home/.cargo/bin:/opt/homebrew/bin:/usr/local/bin"
export PATH

command -v amtr >/dev/null 2>&1 || exit 0

# Everything below this point assumes the binary exists, and so does the store.
# Creating it earlier meant a machine with the plugin installed but no binary
# grew a store and an empty log on every turn, which quietly broke the useful
# inference that a store on disk means this tool has run.
#
# The binary creates the store too, but the redirect below is opened by the
# shell *before* the binary runs. On a fresh install the directory does not
# exist, the redirect fails, and a POSIX shell then skips the entire command —
# so the binary never ran, nothing was created, and the next turn found no
# marker to bootstrap from. The plugin was inert from installation onward, and
# silently: the only complaint went to the hook's own stderr, which is the very
# thing the redirect existed to capture.
#
# The parent is created separately, at the ambient umask. It is `~/.local/share`
# or similar — shared with other tools and not ours to tighten, which is exactly
# what `store::create_dir_private` says on the writer's side. Only the store
# itself goes to 0700, in a subshell so the umask does not leak.
mkdir -p "$(dirname "$amtr_home")" 2>/dev/null
(
	umask 077
	mkdir -p "$amtr_home" 2>/dev/null
	[ -e "$amtr_home/amtr.log" ] || : >>"$amtr_home/amtr.log" 2>/dev/null
	# An older install may have left this readable. `log_stderr_to`'s O_CREAT
	# mode only applies to a file it creates, so an existing one keeps whatever
	# it had — the only thing under the store not brought up to 0600.
	chmod 600 "$amtr_home/amtr.log" 2>/dev/null
) 2>/dev/null

# Resolved once, here, rather than written as a redirect on the commands below.
# A POSIX shell that cannot open a redirect skips the whole command, so with the
# redirect inline, whether the binary ran at all depended on whether this file
# happened to be writable. A store restored from an archive under another owner,
# or a single synthesize that once ran under sudo, was enough to leave the
# plugin permanently silent on an otherwise healthy system. The binary points
# its own stderr at the same file as soon as it starts, so falling back here
# costs a few lines, not the tool.
log="$amtr_home/amtr.log"
{ : >>"$log"; } 2>/dev/null || log=/dev/null

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
	#
	# stderr goes to the log rather than /dev/null. The binary redirects its own
	# stderr there as soon as it can, but it cannot do that before resolving the
	# home directory — and "the home directory would not resolve" is exactly the
	# failure that would otherwise leave no trace at all.
	amtr synthesize "$session_id" "$journal" >/dev/null 2>>"$log"
	;;
recall)
	# The marker is an undelivered snapshot, not a "compaction happened" flag.
	# Its absence means there is nothing owed, which is the normal state of
	# every turn that does not follow a compaction.
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

	# Deliver first, then discharge the debt, so a snapshot is never marked
	# delivered on a turn that failed to inject it. `amtr recall` exits 0 only
	# when it actually printed a handoff — 1 means there was nothing to deliver
	# or it failed, and either way the debt stands.
	#
	# stderr goes to the log, not /dev/null. A row that cannot be parsed fails
	# here on every single turn, and discarding the reason made that invisible:
	# the marker stayed, the turn injected nothing, and nothing anywhere said
	# why.
	if amtr recall "$session_id" 2>>"$log"; then
		# Compare the whole claim, not just its shape. A compaction can finish
		# while this turn is in flight, and its `ready:<newer key>` must not be
		# discharged by the turn that delivered the older one — which a check
		# for "still ready" cannot tell apart.
		if [ "$(cat "$marker" 2>/dev/null)" = "$claim" ]; then
			rm -f "$marker"
		fi
	fi
	;;
esac
exit 0
